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

/// listen 중인 gateway.
#[derive(Debug)]
pub struct Gateway {
    listener: TcpListener,
    registry: Arc<AppRegistry>,
}

impl Gateway {
    /// 주소를 bind한다. 실제 serving은 [`Gateway::serve`]에서 시작한다.
    ///
    /// bind와 serve를 나눈 이유는 테스트가 `port 0`으로 bind한 뒤
    /// 실제 배정된 주소를 읽어야 하기 때문이다.
    pub async fn bind(addr: SocketAddr, registry: Arc<AppRegistry>) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener, registry })
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
                    let service = service_fn(move |request| {
                        handle(Arc::clone(&registry), peer, request)
                    });

                    let connection = http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service);
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

/// 요청 하나를 처리한다.
///
/// 어떤 경로로도 실패를 밖으로 내보내지 않는다. gateway는 항상 응답을 만든다.
async fn handle(
    registry: Arc<AppRegistry>,
    peer: SocketAddr,
    request: Request<Incoming>,
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
    let response = match proxy::forward(lease, request, peer, header_timeout, &mut trace).await {
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
