//! Reverse proxy.
//!
//! 요구사항: PRD §24 HTTP Contract, §26 Acquire와 RequestLease.
//!
//! lease는 **응답 body를 다 흘려보낼 때까지** 살아 있어야 한다.
//! service 함수가 반환하는 순간 반납하면, 아직 backend에서 흘러나오는 중인 응답이 있는데도
//! `active_requests`가 0이 되어 idle shutdown 대상이 된다.
//! 그래서 lease를 응답 body에 실어 보낸다. ([`LeasedBody`])

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http::header::{HeaderName, CONNECTION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE};
use http::{Request, Response};
use http_body::{Body, Frame, SizeHint};
use http_body_util::combinators::BoxBody;
use http_body_util::BodyExt;
use http_body_util::Limited;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tracing::debug;

use crate::app::RequestLease;

/// Gateway가 돌려주는 응답 body 타입.
pub type GatewayBody = BoxBody<Bytes, hyper::Error>;

/// hop-by-hop header. RFC 9110 §7.6.1 — 다음 hop으로 전달하지 않는다.
const HOP_BY_HOP: [HeaderName; 6] = [
    CONNECTION,
    TE,
    TRAILER,
    TRANSFER_ENCODING,
    UPGRADE,
    // `Keep-Alive`와 `Proxy-Authenticate`는 상수가 없어 아래에서 이름으로 처리한다.
    http::header::PROXY_AUTHENTICATE,
];

const HOP_BY_HOP_EXTRA: [&str; 3] = ["keep-alive", "proxy-authorization", "proxy-connection"];

#[derive(Debug, thiserror::Error)]
pub(super) enum ProxyError {
    #[error("failed to connect to backend {addr}: {source}")]
    Connect {
        addr: std::net::SocketAddr,
        #[source]
        source: std::io::Error,
    },

    #[error("backend did not send response headers within {limit:?}")]
    ResponseHeaderTimeout { limit: std::time::Duration },

    #[error("backend handshake failed: {0}")]
    Handshake(#[source] hyper::Error),

    #[error("backend request failed: {0}")]
    Request(#[source] hyper::Error),
}

/// proxy 실패를 상태 코드로 옮긴다.
///
/// 응답이 오지 않은 것(`504`)과 backend를 쓸 수 없는 것(`502`)은 다르다.
/// 앞의 것은 application이 살아 있으나 답하지 않는 상태다. (PRD §26, §75)
pub(super) fn status_for(error: &ProxyError) -> http::StatusCode {
    match error {
        ProxyError::ResponseHeaderTimeout { .. } => http::StatusCode::GATEWAY_TIMEOUT,
        ProxyError::Connect { .. } | ProxyError::Handshake(_) | ProxyError::Request(_) => {
            http::StatusCode::BAD_GATEWAY
        }
    }
}

/// 요청을 backend로 전달하고 응답을 돌려준다.
///
/// 요청마다 새 TCP 연결을 연다. connection pool을 두지 않는 것은 의도적이다.
/// scale-to-zero는 backend를 주기적으로 죽이므로, pool에 남은 keep-alive 연결은
/// 다음 요청에서 그대로 실패한다. pool을 쓰려면 lifecycle 이벤트로 무효화해야 하고
/// 그것은 Core MVP의 범위가 아니다.
///
/// **값을 재 두었다.** 지연은 무시할 만하고, 포트는 생각했던 문제가 아니었다.
///
/// ```text
/// 연결 비용    0.021~0.033ms — ttfb의 2~3% (v0.1.0~v0.1.3 baseline 네 개)
/// TIME_WAIT    요청 2,000개 → backend의 listen 소켓 쪽 1,999개
///                            gateway의 ephemeral 쪽      4개
/// ```
///
/// **TIME_WAIT를 쥐는 쪽은 backend다.** 먼저 닫는 쪽에 남고, backend가 먼저
/// 닫는다. gateway의 ephemeral 포트는 **즉시 풀린다** — 요청마다 연결을 열어도
/// 포트가 고갈되지 않는다. `echo-server`(`connection: close`)와 keep-alive
/// 백엔드에서 모두 같았다.
///
/// 관련 한계는 ephemeral 범위가 아니라 `tcp_max_tw_buckets`다(기본 65,536).
/// 넘기면 커널이 오래된 항목을 재활용하고 경고를 남긴다 — 연결이 실패하지
/// 않는다. bench가 30초간 7,098 rps를 버티는 것이 그 증거다.
///
/// 처음에 "ephemeral 28,232 ÷ 60초 ≈ 470 req/s에서 고갈된다"고 적었다.
/// **틀렸다** — TIME_WAIT를 어느 쪽이 쥐는지 구분하지 않고 "app port와 일치하는
/// 소켓"을 세었다. 그 수치는 거의 전부 backend 쪽이었다.
pub(super) async fn forward(
    lease: RequestLease,
    request: Request<Incoming>,
    peer: std::net::SocketAddr,
    header_timeout: Duration,
    max_body_bytes: Option<u64>,
    trace: &mut crate::trace::RequestTrace,
) -> Result<Response<GatewayBody>, ProxyError> {
    let addr = lease.backend().addr();

    // 상한은 **응답 header가 돌아오기까지만** 적용한다. (PRD §75)
    //
    // body에는 걸지 않는다. 큰 파일이나 천천히 흐르는 응답을 끊지 않기 위해서다.
    // `send_request`는 header가 도착하면 반환하고 body는 그 뒤에 흐르므로,
    // 이 구간을 감싸는 것이 곧 "header까지"다.
    //
    // 전체 요청에 상한을 두면 어떤 기본값이든 누군가의 정상 요청을 깨뜨린다.
    // 막으려는 것은 느린 응답이 아니라 **오지 않는 응답**이다.
    // 연결 수립 시각을 블록 밖으로 내보낸다. `trace`를 블록이 붙잡으면 await를
    // 넘는 가변 대여가 되므로, 값으로 돌려받아 밖에서 기록한다. (PRD §72)
    let (connected_at, response) = tokio::time::timeout(header_timeout, async {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|source| ProxyError::Connect { addr, source })?;
        let connected_at = tokio::time::Instant::now();

        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(ProxyError::Handshake)?;

        // connection task는 응답 body가 다 흐를 때까지 살아 있어야 한다.
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                debug!(%addr, %error, "backend connection closed");
            }
        });

        let response = sender
            .send_request(to_backend_request(request, peer, max_body_bytes))
            .await
            .map_err(ProxyError::Request)?;

        Ok::<_, ProxyError>((connected_at, response))
    })
    .await
    .map_err(|_| ProxyError::ResponseHeaderTimeout {
        limit: header_timeout,
    })??;

    trace.backend_connected_at(connected_at);

    let (parts, body) = response.into_parts();
    let mut response = Response::from_parts(parts, LeasedBody { inner: body, lease }.boxed());
    strip_hop_by_hop(response.headers_mut());

    Ok(response)
}

/// 들어온 요청을 backend로 보낼 형태로 바꾼다.
///
/// `Host` header는 그대로 넘긴다. application이 자기 host를 볼 수 있어야 한다.
/// backend로 보낼 요청. 본문은 상한을 세면서 흘린다. (PRD §83)
///
/// **상한이 꺼져 있어도 `Limited`로 감싼다.** `usize::MAX`를 주면 사실상 무제한
/// 이고, 경로를 하나로 유지하는 편이 두 갈래를 두는 것보다 틀릴 여지가 적다.
fn to_backend_request(
    request: Request<Incoming>,
    peer: std::net::SocketAddr,
    max_body_bytes: Option<u64>,
) -> Request<Limited<Incoming>> {
    let (mut parts, body) = request.into_parts();

    // client conn은 origin-form을 기대한다. absolute-form으로 들어온 요청을 정규화한다.
    if let Some(path_and_query) = parts.uri.path_and_query().cloned() {
        let mut uri = http::uri::Parts::default();
        uri.path_and_query = Some(path_and_query);
        if let Ok(origin_form) = http::Uri::from_parts(uri) {
            parts.uri = origin_form;
        }
    }

    strip_hop_by_hop(&mut parts.headers);
    append_forwarded_for(&mut parts.headers, peer);

    // `Content-Length`가 선언한 크기는 gateway가 이미 봤다. 여기는 **흐르면서
    // 넘는 경우**(chunked)를 위한 것이다. 넘으면 body가 오류를 내고 전송이
    // 끊긴다 — 그때는 backend가 일부를 받은 뒤라 깨끗한 `413`을 줄 수 없다.
    let limit = max_body_bytes.map_or(usize::MAX, |bytes| {
        usize::try_from(bytes).unwrap_or(usize::MAX)
    });
    Request::from_parts(parts, Limited::new(body, limit))
}

/// 관측한 peer 주소를 `X-Forwarded-For`에 덧붙인다. (PRD §81)
///
/// 기존 값이 있으면 그 뒤에, 없으면 하나로 만든다. 표준 reverse proxy의 동작이다.
///
/// **왼쪽의 값들을 검증하지 않는다.** 직접 노출된 상태에서 클라이언트가 이 header를
/// 보내면 그것이 그대로 남고 그 뒤에 진짜 peer가 붙는다. **맨 오른쪽만 gateway가
/// 직접 관측한 값이다.**
///
/// port는 넣지 않는다. 이 header는 주소 목록이고 port는 그 문법에 없다.
fn append_forwarded_for(headers: &mut http::HeaderMap, peer: std::net::SocketAddr) {
    let name = http::header::HeaderName::from_static("x-forwarded-for");
    let observed = peer.ip().to_string();

    // header가 여러 줄로 올 수 있다. 전부 모아 한 줄로 합친 뒤 우리 것을 붙인다.
    let existing: Vec<String> = headers
        .get_all(&name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect();

    let chain = if existing.is_empty() {
        observed
    } else {
        format!("{}, {observed}", existing.join(", "))
    };

    // 값에 쓸 수 없는 문자가 섞여 들어오면 우리 것만 남긴다. 조용히 지우지 않는다.
    match http::HeaderValue::from_str(&chain) {
        Ok(value) => {
            headers.insert(&name, value);
        }
        Err(_) => {
            if let Ok(value) = http::HeaderValue::from_str(&peer.ip().to_string()) {
                headers.insert(&name, value);
            }
        }
    }
}

fn strip_hop_by_hop(headers: &mut http::HeaderMap) {
    for name in HOP_BY_HOP {
        headers.remove(name);
    }
    for name in HOP_BY_HOP_EXTRA {
        headers.remove(name);
    }
}

/// [`RequestLease`]를 함께 들고 다니는 body.
///
/// body가 끝나거나 drop될 때 lease도 함께 drop된다. 클라이언트가 중간에 끊어도
/// body가 drop되므로 `active_requests`는 반드시 회수된다. (PRD §26)
struct LeasedBody<B> {
    inner: B,
    lease: RequestLease,
}

impl<B> Body for LeasedBody<B>
where
    B: Body + Unpin,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.get_mut().inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl<B> std::fmt::Debug for LeasedBody<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeasedBody")
            .field("lease", &self.lease.id())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    fn headers_with(values: &[&str]) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        for value in values {
            headers.append(
                http::header::HeaderName::from_static("x-forwarded-for"),
                http::HeaderValue::from_str(value).expect("test value is valid"),
            );
        }
        headers
    }

    fn forwarded(headers: &http::HeaderMap) -> String {
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    fn peer(addr: &str) -> SocketAddr {
        addr.parse().expect("test address parses")
    }

    /// 없으면 관측한 peer 하나로 만든다. (PRD §81)
    ///
    /// 직접 노출된 gateway에서 이것이 application이 접속자를 아는 유일한 길이다.
    #[test]
    fn it_sets_the_peer_when_there_is_none() {
        let mut headers = http::HeaderMap::new();
        append_forwarded_for(&mut headers, peer("203.0.113.7:51234"));
        assert_eq!(forwarded(&headers), "203.0.113.7");
    }

    /// port는 넣지 않는다. 이 header는 주소 목록이다. (PRD §81)
    #[test]
    fn it_does_not_include_the_port() {
        let mut headers = http::HeaderMap::new();
        append_forwarded_for(&mut headers, peer("203.0.113.7:51234"));
        assert!(
            !forwarded(&headers).contains("51234"),
            "port가 들어갔다: {}",
            forwarded(&headers),
        );
    }

    /// 기존 값 **뒤에** 붙인다. 맨 오른쪽이 우리가 관측한 것이다. (PRD §81)
    #[test]
    fn it_appends_to_the_right() {
        let mut headers = headers_with(&["1.2.3.4"]);
        append_forwarded_for(&mut headers, peer("10.0.0.5:4000"));
        assert_eq!(forwarded(&headers), "1.2.3.4, 10.0.0.5");
    }

    /// 여러 줄로 온 header를 한 줄로 합친다. (PRD §81)
    ///
    /// HTTP는 같은 header를 여러 번 보낼 수 있다. 하나만 보고 덮어쓰면 앞의 hop이
    /// 사라진다.
    #[test]
    fn it_merges_repeated_headers() {
        let mut headers = headers_with(&["1.2.3.4", "5.6.7.8"]);
        append_forwarded_for(&mut headers, peer("10.0.0.5:4000"));
        assert_eq!(forwarded(&headers), "1.2.3.4, 5.6.7.8, 10.0.0.5");
        assert_eq!(
            headers.get_all("x-forwarded-for").iter().count(),
            1,
            "한 줄로 합쳐야 한다",
        );
    }

    /// IPv6도 그대로 적는다.
    #[test]
    fn it_handles_ipv6() {
        let mut headers = http::HeaderMap::new();
        append_forwarded_for(&mut headers, peer("[2001:db8::1]:8080"));
        assert_eq!(forwarded(&headers), "2001:db8::1");
    }

    /// 빈 값은 무시한다. 쉼표만 남은 목록을 만들지 않는다.
    #[test]
    fn it_ignores_empty_values() {
        let mut headers = headers_with(&["", "  "]);
        append_forwarded_for(&mut headers, peer("10.0.0.5:4000"));
        assert_eq!(forwarded(&headers), "10.0.0.5");
    }

    /// `X-Forwarded-Proto`는 건드리지 않는다. Core는 TLS를 제공하지 않는다. (PRD §81)
    #[test]
    fn it_leaves_forwarded_proto_alone() {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::HeaderName::from_static("x-forwarded-proto"),
            http::HeaderValue::from_static("https"),
        );
        append_forwarded_for(&mut headers, peer("10.0.0.5:4000"));
        assert_eq!(
            headers
                .get("x-forwarded-proto")
                .and_then(|value| value.to_str().ok()),
            Some("https"),
            "상위 프록시가 적은 것을 지우면 안 된다",
        );
    }
}
