//! HTTP Gateway.
//!
//! 요구사항: PRD §23–§26, §69.
//!
//! routing chain:
//!
//! ```text
//! Host → AppRegistry → AppHandle → acquire() → RequestLease → Reverse Proxy
//! ```
//!
//! Gateway는 `Runtime`에 직접 접근하지 않고 application의 상태도 검사하지 않는다.
//! lifecycle에 대해 아는 것은 `acquire()` 하나뿐이다. (Invariant 3)

mod proxy;
mod router;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tracing::{debug, info, warn};

use crate::registry::AppRegistry;
use crate::trace::RequestTrace;

pub use proxy::GatewayBody;

/// graceful shutdown이 진행 중인 연결을 기다리는 최대 시간.
///
/// keep-alive 연결을 붙잡고 아무것도 보내지 않는 client 때문에 daemon 종료가
/// 무한정 늦어져서는 안 된다.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

/// 요청을 **받는 쪽**의 상한. (PRD §83)
///
/// `crate::Limits`와 다른 것이다 — 저쪽은 "app을 깨워도 되는가"이고 이쪽은
/// "이 요청을 끝까지 받을 것인가"다. 그래서 기본값의 방침도 다르다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLimits {
    /// 요청 header를 다 받기까지의 상한. `None`이면 끈다.
    ///
    /// **기본값이 있다.** 정상 클라이언트가 header를 보내는 데 15초가 걸릴
    /// 이유가 없고, 끊어서 곤란해지는 쪽이 없다. 느린 header는 어느 app으로
    /// 갈지 모르는 상태에서 Core의 task와 socket을 붙잡는다 — slowloris다.
    pub header_read_timeout: Option<Duration>,
    /// 요청 본문의 최대 바이트. `None`이면 끈다.
    ///
    /// **기본이 끔이다.** 어떤 기본값이든 누군가의 정상 업로드를 깨뜨린다.
    /// gateway는 본문을 버퍼링하지 않고 흘리므로 Core 자신은 위험하지 않다 —
    /// 막아 주는 대상은 application이다. (PRD §83)
    pub max_request_body_bytes: Option<u64>,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            header_read_timeout: Some(Duration::from_secs(15)),
            max_request_body_bytes: None,
        }
    }
}

/// listen 중인 gateway.
#[derive(Debug)]
pub struct Gateway {
    listener: TcpListener,
    registry: Arc<AppRegistry>,
    limits: RequestLimits,
}

impl Gateway {
    /// 주소를 bind한다. 실제 serving은 [`Gateway::serve`]에서 시작한다.
    ///
    /// bind와 serve를 나눈 이유는 테스트가 `port 0`으로 bind한 뒤
    /// 실제 배정된 주소를 읽어야 하기 때문이다.
    pub async fn bind(
        addr: SocketAddr,
        registry: Arc<AppRegistry>,
        limits: RequestLimits,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self {
            listener,
            registry,
            limits,
        })
    }

    /// 실제로 bind된 주소.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// `shutdown`이 완료될 때까지 요청을 받는다.
    ///
    /// 종료 신호가 오면 새 연결 수락을 멈추고, 진행 중인 연결은 마무리되도록 기다린다.
    /// application을 정지시키는 것은 호출자의 몫이다. ([`AppRegistry::shutdown`])
    pub async fn serve(self, shutdown: impl std::future::Future<Output = ()> + Send) {
        let graceful = GracefulShutdown::new();
        let mut shutdown = std::pin::pin!(shutdown);

        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let (stream, peer) = match accepted {
                        Ok(accepted) => accepted,
                        // accept 실패 하나로 gateway 전체를 내리지 않는다.
                        // fd 고갈 같은 일시적 상황에서 다음 연결은 성공할 수 있다.
                        Err(error) => {
                            warn!(%error, "failed to accept a connection");
                            continue;
                        }
                    };

                    let registry = Arc::clone(&self.registry);
                    // peer를 handler까지 흘린다. backend에게 누가 접속했는지
                    // 알려 주는 유일한 출처다. (PRD §81)
                    let limits = self.limits;
                    let service = service_fn(move |request| {
                        handle(Arc::clone(&registry), peer, request, limits)
                    });

                    // **`timer`를 함께 설정해야 한다.** (PRD §83)
                    //
                    // hyper의 `header_read_timeout`은 `Timer` 없이는 동작하지
                    // 않는다. 기본값 30초가 문서에 적혀 있지만 timer를 주지
                    // 않으면 적용되지 않는다 — 여기에 timer가 없어서 지금까지
                    // header 읽기에 **아무 상한이 없었다.**
                    let mut builder = http1::Builder::new();
                    builder.timer(hyper_util::rt::TokioTimer::new());
                    builder.header_read_timeout(limits.header_read_timeout);
                    let connection = builder.serve_connection(TokioIo::new(stream), service);
                    let connection = graceful.watch(connection);

                    tokio::spawn(async move {
                        if let Err(error) = connection.await {
                            debug!(%peer, %error, "connection ended");
                        }
                    });
                }

                () = &mut shutdown => {
                    info!("gateway is shutting down, no longer accepting connections");
                    break;
                }
            }
        }

        if tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, graceful.shutdown())
            .await
            .is_err()
        {
            warn!(
                timeout = ?GRACEFUL_SHUTDOWN_TIMEOUT,
                "some connections did not finish in time, dropping them",
            );
        }
    }
}

/// WebSocket 핸드셰이크인가. (PRD §84)
///
/// `Upgrade: websocket`만 본다. 대소문자를 구분하지 않는다 — header 값의
/// 토큰은 대소문자를 가리지 않고, 실제로 `WebSocket`으로 보내는 클라이언트가 있다.
///
/// 값이 여럿일 수 있다(`Upgrade: websocket, h2c`). 하나라도 websocket이면 맞다.
fn requests_websocket(request: &Request<Incoming>) -> bool {
    request
        .headers()
        .get_all(http::header::UPGRADE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("websocket"))
}

/// `Content-Length`가 선언한 본문 크기. 없거나 읽을 수 없으면 `None`.
///
/// 값이 여럿이거나 숫자가 아니면 `None`을 돌려준다 — 그런 요청은 hyper가
/// 먼저 거부하거나 chunked로 다루므로, 여기서 판단하지 않고 흐름 검사에 맡긴다.
fn declared_body_length(request: &Request<Incoming>) -> Option<u64> {
    request
        .headers()
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// 요청 하나를 처리한다.
///
/// 어떤 경로로도 실패를 밖으로 내보내지 않는다. gateway는 항상 응답을 만든다.
async fn handle(
    registry: Arc<AppRegistry>,
    peer: SocketAddr,
    request: Request<Incoming>,
    limits: RequestLimits,
) -> Result<Response<GatewayBody>, Infallible> {
    let mut trace = RequestTrace::received();

    let Some(host) = router::host_of(&request).map(ToOwned::to_owned) else {
        return Ok(finish(
            &mut trace,
            "-",
            router::text(StatusCode::BAD_REQUEST, "missing Host header"),
        ));
    };

    let Some(app) = registry.resolve(&host) else {
        debug!(%host, "no application is registered for this host");
        return Ok(finish(
            &mut trace,
            "-",
            router::text(
                StatusCode::NOT_FOUND,
                "no application is registered for this host",
            ),
        ));
    };

    // **WebSocket upgrade는 여기서 거절한다.** (PRD §84)
    //
    // 지우고 넘기면 backend가 평범한 GET으로 보고 200이나 400을 돌려준다.
    // 클라이언트는 "handshake failed"만 보고 왜인지 알 수 없다 — 앱을 의심하고,
    // 프레임워크를 의심하고, 터널을 의심한 다음에야 gateway에 닿는다.
    //
    // `websocket`만 본다. `h2c` 같은 다른 토큰은 지금처럼 지우고 넘긴다.
    if requests_websocket(&request) {
        warn!(
            app = app.name(),
            "refusing a websocket upgrade; this gateway does not implement it (PRD §24)",
        );
        return Ok(finish(
            &mut trace,
            app.name(),
            router::text(
                StatusCode::NOT_IMPLEMENTED,
                "this gateway does not implement websocket upgrades",
            ),
        ));
    }

    // **깨우기 전에 판단한다.** (PRD §82)
    //
    // 거절할 요청이 process를 띄우면 그것만으로 자원 고갈 수단이 된다. §78의
    // 메모리 입장 제어와 같은 자리다.
    //
    // gateway가 직접 관측한 TCP peer만 본다. `X-Forwarded-For`는 보지 않는다 —
    // 클라이언트가 보낼 수 있어서 한 줄로 우회된다 (§81).
    if !app.spec().server.allows(peer.ip()) {
        warn!(
            app = app.name(),
            peer = %peer.ip(),
            "peer is not allowed to reach this application",
        );
        return Ok(finish(
            &mut trace,
            app.name(),
            router::text(StatusCode::FORBIDDEN, "forbidden"),
        ));
    }

    // **선언된 본문이 상한을 넘으면 여기서 끝낸다.** (PRD §83)
    //
    // backend에 접속하기도, app을 깨우기도 전이다 — §82·§78과 같은 자리다.
    // `Content-Length`가 없는 chunked 요청은 흘려 보면서 센다 (proxy).
    if let Some(limit) = limits.max_request_body_bytes {
        if let Some(declared) = declared_body_length(&request) {
            if declared > limit {
                warn!(
                    app = app.name(),
                    declared, limit, "request body exceeds the configured limit",
                );
                return Ok(finish(
                    &mut trace,
                    app.name(),
                    router::text(StatusCode::PAYLOAD_TOO_LARGE, "request body is too large"),
                ));
            }
        }
    }

    // 상태를 보지 않고 acquire만 한다. SLEEPING이면 여기서 깨어난다. (Invariant 3)
    trace.acquire_started();
    let lease = match app.acquire().await {
        Ok(lease) => lease,
        Err(error) => {
            let response = router::response_for(&error);
            warn!(app = app.name(), status = %response.status(), %error, "acquire failed");
            return Ok(finish(&mut trace, app.name(), response));
        }
    };
    trace.acquire_completed();

    trace.proxy_started();
    let header_timeout = app.spec().lifecycle.response_header_timeout();
    let response = match proxy::forward(
        lease,
        request,
        peer,
        header_timeout,
        limits.max_request_body_bytes,
        &mut trace,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            let status = proxy::status_for(&error);
            warn!(app = app.name(), %status, %error, "proxy failed");
            router::text(status, &error.to_string())
        }
    };

    Ok(finish(&mut trace, app.name(), response))
}

/// 응답 header가 나가는 시점을 찍고 구간을 흘린다. (PRD §72)
fn finish(
    trace: &mut RequestTrace,
    app: &str,
    response: Response<GatewayBody>,
) -> Response<GatewayBody> {
    trace.response_started();
    trace.emit(app, response.status().as_u16());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **header 읽기 상한은 기본으로 켜져 있다.** (PRD §83)
    ///
    /// §83에서 유일하게 기본값을 가진 상한이다. 끄면 slowloris가 그대로 통한다.
    /// e2e는 `--header-read-timeout-seconds`를 명시해서 돌기 때문에 기본값이
    /// 바뀐 것을 잡지 못한다 — 그래서 여기서 못 박는다.
    #[test]
    fn the_header_read_timeout_is_on_by_default() {
        assert_eq!(
            RequestLimits::default().header_read_timeout,
            Some(Duration::from_secs(15)),
            "기본으로 켜져 있어야 한다",
        );
    }

    /// **본문 상한은 기본으로 꺼져 있다.** (PRD §83)
    ///
    /// 어떤 기본값이든 누군가의 정상 업로드를 깨뜨린다.
    #[test]
    fn the_body_limit_is_off_by_default() {
        assert_eq!(RequestLimits::default().max_request_body_bytes, None);
    }
}
