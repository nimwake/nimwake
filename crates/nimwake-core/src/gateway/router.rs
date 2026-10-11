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
///
/// backoff도 `503`이지만 `Retry-After`가 붙는다. (PRD §77)
/// overload는 바로 다시 보내면 될 수 있고, backoff는 남은 시간 동안 소용이 없다.
pub(super) const fn status_for(error: &AcquireError) -> StatusCode {
    match error {
        AcquireError::Overloaded | AcquireError::Unavailable | AcquireError::BackingOff { .. } => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        AcquireError::StartFailed(_) => StatusCode::BAD_GATEWAY,
    }
}

/// `acquire()` 실패를 응답으로 옮긴다.
///
/// backoff일 때만 `Retry-After`를 붙인다. 그것이 overload와 가르는 신호다. (PRD §77)
pub(super) fn response_for(error: &AcquireError) -> Response<GatewayBody> {
    let mut response = text(status_for(error), &error.to_string());

    if let AcquireError::BackingOff { retry_after } = error {
        // `Retry-After`는 초 단위 정수다. 1초 미만을 0으로 내리면 "바로 다시 보내라"가
        // 되어 뜻이 뒤집힌다. 올린다.
        let seconds = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
        if let Ok(value) = http::HeaderValue::from_str(&seconds.to_string()) {
            response
                .headers_mut()
                .insert(http::header::RETRY_AFTER, value);
        }
    }

    response
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::error::AppError;

    /// backoff에만 `Retry-After`가 붙는다. 그것이 overload와 가르는 신호다. (PRD §77)
    #[test]
    fn only_backoff_carries_retry_after() {
        let backoff = response_for(&AcquireError::BackingOff {
            retry_after: Duration::from_secs(4),
        });
        assert_eq!(backoff.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            backoff
                .headers()
                .get(http::header::RETRY_AFTER)
                .map(|value| value.to_str().unwrap_or_default()),
            Some("4"),
        );

        // overload도 503이지만 Retry-After는 없다. 바로 다시 보내면 될 수 있다.
        let overloaded = response_for(&AcquireError::Overloaded);
        assert_eq!(overloaded.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            overloaded
                .headers()
                .get(http::header::RETRY_AFTER)
                .is_none(),
            "overload에 Retry-After를 붙이면 둘을 구분할 수 없다",
        );

        // 기동 실패는 502다. 시도했고 실패했다는 뜻이다.
        let failed = response_for(&AcquireError::StartFailed(AppError::StartupTimeout(
            Duration::from_secs(5),
        )));
        assert_eq!(failed.status(), StatusCode::BAD_GATEWAY);
        assert!(failed.headers().get(http::header::RETRY_AFTER).is_none());
    }

    /// 1초 미만을 `0`으로 내리면 "바로 다시 보내라"가 되어 뜻이 뒤집힌다. (PRD §77)
    #[test]
    fn a_sub_second_delay_rounds_up() {
        let response = response_for(&AcquireError::BackingOff {
            retry_after: Duration::from_millis(1),
        });
        assert_eq!(
            response
                .headers()
                .get(http::header::RETRY_AFTER)
                .map(|value| value.to_str().unwrap_or_default()),
            Some("1"),
            "0으로 내리면 즉시 재시도하라는 뜻이 된다",
        );
    }
}
