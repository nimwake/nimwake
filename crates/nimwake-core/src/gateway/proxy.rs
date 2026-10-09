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
pub(super) async fn forward(
    lease: RequestLease,
    request: Request<Incoming>,
    header_timeout: Duration,
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
    let response = tokio::time::timeout(header_timeout, async {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|source| ProxyError::Connect { addr, source })?;

        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(ProxyError::Handshake)?;

        // connection task는 응답 body가 다 흐를 때까지 살아 있어야 한다.
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                debug!(%addr, %error, "backend connection closed");
            }
        });

        sender
            .send_request(to_backend_request(request))
            .await
            .map_err(ProxyError::Request)
    })
    .await
    .map_err(|_| ProxyError::ResponseHeaderTimeout {
        limit: header_timeout,
    })??;

    let (parts, body) = response.into_parts();
    let mut response = Response::from_parts(parts, LeasedBody { inner: body, lease }.boxed());
    strip_hop_by_hop(response.headers_mut());

    Ok(response)
}

/// 들어온 요청을 backend로 보낼 형태로 바꾼다.
///
/// `Host` header는 그대로 넘긴다. application이 자기 host를 볼 수 있어야 한다.
fn to_backend_request(request: Request<Incoming>) -> Request<Incoming> {
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
    Request::from_parts(parts, body)
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
