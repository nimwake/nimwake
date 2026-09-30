//! 종료 코드.
//!
//! 요구사항: PRD §74 Host Interface.
//!
//! supervisor는 daemon이 죽으면 재기동한다. 그러나 **재시도해도 나아지지 않는 실패**에서는
//! 멈추고 operator에게 넘겨야 한다. 종료 코드로 그것을 구분한다.
//!
//! 이 값이 없으면 supervisor는 stderr 문자열을 파싱하거나
//! "기동 직후 연속 종료" 같은 휴리스틱에 기대야 한다.
//!
//! `sysexits` 관례를 따른다.
//!
//! serving은 Unix에서만 한다. 다른 플랫폼은 manifest 검증까지만 수행하므로
//! 설정 오류 말고는 닿는 실패가 없다. 그래도 계약 표는 플랫폼과 무관하게 한곳에 둔다.

#![cfg_attr(not(unix), allow(dead_code))]

use std::process::ExitCode;

/// 설정 오류. 사람이 고쳐야 한다. (`EX_CONFIG`)
const CONFIG: u8 = 78;

/// 지금은 쓸 수 없지만 시간이 해결할 수 있다. (`EX_UNAVAILABLE`)
const UNAVAILABLE: u8 = 69;

/// 그 밖의 실패.
const FAILURE: u8 = 1;

/// 실패 원인과, 그것을 supervisor에게 알리는 종료 코드.
#[derive(Debug, PartialEq, Eq)]
pub struct Failure {
    code: u8,
    message: String,
}

impl Failure {
    /// manifest·설정 오류. supervisor는 재기동하지 않고 `UNHEALTHY`로 표시한다.
    pub fn config(message: impl Into<String>) -> Self {
        Self {
            code: CONFIG,
            message: message.into(),
        }
    }

    /// listen 주소를 지금 쓸 수 없다. 이전 인스턴스가 아직 쥐고 있을 수 있다.
    /// supervisor는 backoff 후 재시도한다.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            code: UNAVAILABLE,
            message: message.into(),
        }
    }

    /// 분류되지 않은 실패.
    pub fn other(message: impl Into<String>) -> Self {
        Self {
            code: FAILURE,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> u8 {
        self.code
    }

    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(self.code)
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configuration_failure_tells_the_supervisor_not_to_retry() {
        assert_eq!(Failure::config("bad manifest").code(), 78);
    }

    #[test]
    fn an_unavailable_failure_is_worth_retrying() {
        assert_eq!(Failure::unavailable("port in use").code(), 69);
    }

    #[test]
    fn anything_else_falls_back_to_one() {
        assert_eq!(Failure::other("boom").code(), 1);
    }

    #[test]
    fn the_message_survives() {
        assert_eq!(Failure::config("no manifests").to_string(), "no manifests");
    }
}
