//! Host 기반 routing과 상태 코드 매핑.
//!
//! 요구사항: PRD §23 Gateway Routing, §69 Backpressure.
//!
//! Gateway는 application의 현재 상태를 검사하지 않는다.
//! RUNNING이든 SLEEPING이든 `acquire()` 하나만 부르고, wake 여부는 AppManager가 판단한다.
//! (Invariant 3)

use bytes::Bytes;
use http::header::{CONTENT_TYPE, HOST};
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};

use crate::error::AcquireError;

use super::proxy::GatewayBody;

/// 요청에서 routing에 쓸 host를 뽑는다.
///
/// absolute-form 요청은 URI authority가, 그 외에는 `Host` header가 근거다.
pub(super) fn host_of<B>(request: &Request<B>) -> Option<&str> {
    if let Some(authority) = request.uri().authority() {
        return Some(authority.as_str());
    }

    request.headers().get(HOST)?.to_str().ok()
}

/// `acquire()` 실패를 HTTP 상태로 옮긴다.
///
/// overload는 `503`이다. (PRD §69)
/// 기동 실패는 upstream이 응답할 수 없다는 뜻이므로 `502`로 구분한다.
/// 둘을 같은 코드로 뭉개면 운영자가 "부하 때문인지 app이 깨진 것인지"를 로그 없이 알 수 없다.
pub(super) const fn status_for(error: &AcquireError) -> StatusCode {
    match error {
        AcquireError::Overloaded | AcquireError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        AcquireError::StartFailed(_) => StatusCode::BAD_GATEWAY,
    }
}

/// 본문이 한 줄뿐인 응답.
pub(super) fn text(status: StatusCode, message: &str) -> Response<GatewayBody> {
    let body = Full::new(Bytes::from(format!("{message}\n")))
        .map_err(|never| match never {})
        .boxed();

    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(body)
        .unwrap_or_else(|_| {
            // builder는 header와 status가 유효하면 실패하지 않는다. 둘 다 여기서 만든 값이다.
            let mut fallback = Response::new(empty());
            *fallback.status_mut() = status;
            fallback
        })
}

fn empty() -> GatewayBody {
    Full::new(Bytes::new())
        .map_err(|never| match never {})
        .boxed()
}
