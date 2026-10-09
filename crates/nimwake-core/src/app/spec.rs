//! Application manifest(`nimwake.toml`)와 domain model.
//!
//! 요구사항: PRD §21 Application Manifest.
//!
//! manifest는 특정 runtime이 아니라 동작을 기술한다.
//! 계약은 한 문장이다: "이 command를 실행하면 이 port에서 HTTP application이 listen한다."

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::{ManifestError, ValidationError};

/// pending waiter 기본 상한. app별로 manifest에서 재정의할 수 있다.
pub const DEFAULT_MAX_PENDING_REQUESTS: usize = 256;

const DEFAULT_IDLE_TIMEOUT_SECONDS: u64 = 60;
const DEFAULT_STARTUP_TIMEOUT_SECONDS: u64 = 5;
const DEFAULT_SHUTDOWN_TIMEOUT_SECONDS: u64 = 5;

/// 응답 header를 기다리는 기본 상한. (PRD §75)
///
/// 넉넉하게 잡는다. 이 상한이 겨냥하는 것은 "느린 application"이 아니라
/// "응답하지 않는 application"이다. 긴 DB 조회나 무거운 SSR도 header가 늦으므로,
/// 짧게 잡으면 멀쩡한 요청을 끊는다.
const DEFAULT_RESPONSE_HEADER_TIMEOUT_SECONDS: u64 = 60;

/// 하나의 application 정의.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSpec {
    pub name: String,
    pub server: ServerSpec,
    pub runtime: RuntimeSpec,
    #[serde(default)]
    pub lifecycle: LifecycleSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpec {
    /// routing에 사용하는 `Host` header 값.
    pub host: String,
    /// application이 listen하는 loopback port.
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSpec {
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    /// app별 environment. daemon의 환경 위에 덮어쓴다. (PRD §20)
    ///
    /// `PATH` 같은 기본 환경까지 지우면 대부분의 interpreter가 실행되지 않으므로
    /// MVP는 전체 격리가 아니라 overlay로 둔다. 강한 격리는 container runtime의 주제다.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LifecycleSpec {
    pub idle_timeout_seconds: u64,
    pub startup_timeout_seconds: u64,
    pub shutdown_timeout_seconds: u64,
    /// 응답 header를 기다리는 상한. 초과하면 `504`로 끝낸다. (PRD §75)
    ///
    /// **body 전송에는 적용하지 않는다.** 큰 파일이나 천천히 흐르는 응답을
    /// 끊지 않기 위한 것이다. 제한하는 구간은 요청을 보내고 응답의 첫 줄이
    /// 돌아오기까지다.
    pub response_header_timeout_seconds: u64,
    /// startup을 기다릴 수 있는 최대 waiter 수. 초과분은 `503`으로 거절한다.
    pub max_pending_requests: usize,
}

impl Default for LifecycleSpec {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: DEFAULT_IDLE_TIMEOUT_SECONDS,
            startup_timeout_seconds: DEFAULT_STARTUP_TIMEOUT_SECONDS,
            shutdown_timeout_seconds: DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
            response_header_timeout_seconds: DEFAULT_RESPONSE_HEADER_TIMEOUT_SECONDS,
            max_pending_requests: DEFAULT_MAX_PENDING_REQUESTS,
        }
    }
}

impl LifecycleSpec {
    #[must_use]
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(self.idle_timeout_seconds)
    }

    #[must_use]
    pub fn startup_timeout(&self) -> Duration {
        Duration::from_secs(self.startup_timeout_seconds)
    }

    #[must_use]
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.shutdown_timeout_seconds)
    }

    #[must_use]
    pub fn response_header_timeout(&self) -> Duration {
        Duration::from_secs(self.response_header_timeout_seconds)
    }
}

impl AppSpec {
    /// TOML 문자열에서 manifest를 읽는다. 단일 app 수준의 검증까지 수행한다.
    pub fn from_toml(source: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(source)
    }

    /// manifest 파일을 읽어 파싱하고 단일 app 검증을 수행한다.
    ///
    /// 상대 경로 `working_dir`은 프로세스의 현재 디렉터리가 아니라
    /// **manifest 파일이 있는 디렉터리** 기준으로 해석한다.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path).map_err(|source| ManifestError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let mut spec: Self = toml::from_str(&source).map_err(|source| ManifestError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;

        if spec.runtime.working_dir.is_relative() {
            if let Some(base) = path.parent() {
                spec.runtime.working_dir = base.join(&spec.runtime.working_dir);
            }
        }

        spec.validate()?;
        Ok(spec)
    }

    /// 다른 app과 무관하게 판정할 수 있는 검증.
    ///
    /// 중복(name / host / port)과 gateway port 충돌은 registry 수준에서 검증한다.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let app = || self.name.clone();

        if self.name.trim().is_empty() {
            return Err(ValidationError::EmptyName { app: app() });
        }
        if self.server.host.trim().is_empty() {
            return Err(ValidationError::EmptyHost { app: app() });
        }
        if self.runtime.command.is_empty() || self.runtime.command[0].trim().is_empty() {
            return Err(ValidationError::EmptyCommand { app: app() });
        }
        if !self.runtime.working_dir.is_dir() {
            return Err(ValidationError::MissingWorkingDir {
                app: app(),
                path: self.runtime.working_dir.clone(),
            });
        }

        for (value, field) in [
            (self.lifecycle.idle_timeout_seconds, "idle_timeout_seconds"),
            (
                self.lifecycle.startup_timeout_seconds,
                "startup_timeout_seconds",
            ),
            (
                self.lifecycle.shutdown_timeout_seconds,
                "shutdown_timeout_seconds",
            ),
        ] {
            if value == 0 {
                return Err(ValidationError::NotPositive { app: app(), field });
            }
        }

        if self.lifecycle.max_pending_requests == 0 {
            return Err(ValidationError::NotPositive {
                app: app(),
                field: "max_pending_requests",
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
name = "hello"

[server]
host = "hello.localhost"
port = 3101

[runtime]
command = ["node", "server.js"]
working_dir = "."
"#;

    #[test]
    fn parses_manifest_with_default_lifecycle() {
        let spec = AppSpec::from_toml(MANIFEST).expect("manifest parses");
        assert_eq!(spec.name, "hello");
        assert_eq!(spec.server.port, 3101);
        assert_eq!(spec.lifecycle, LifecycleSpec::default());
        assert_eq!(spec.lifecycle.idle_timeout(), Duration::from_secs(60));
    }

    #[test]
    fn rejects_unknown_fields() {
        let source = format!("{MANIFEST}\n[lifecycle]\nidle_timeout_secondz = 1\n");
        assert!(AppSpec::from_toml(&source).is_err());
    }

    #[test]
    fn rejects_empty_command() {
        let source = MANIFEST.replace(r#"command = ["node", "server.js"]"#, "command = []");
        let spec = AppSpec::from_toml(&source).expect("manifest parses");
        assert_eq!(
            spec.validate(),
            Err(ValidationError::EmptyCommand {
                app: "hello".to_owned()
            })
        );
    }

    #[test]
    fn rejects_missing_working_dir() {
        let source = MANIFEST.replace(r#"working_dir = ".""#, r#"working_dir = "./__nope__""#);
        let spec = AppSpec::from_toml(&source).expect("manifest parses");
        assert!(matches!(
            spec.validate(),
            Err(ValidationError::MissingWorkingDir { .. })
        ));
    }

    #[test]
    fn rejects_zero_timeout() {
        let source = format!("{MANIFEST}\n[lifecycle]\nidle_timeout_seconds = 0\n");
        let spec = AppSpec::from_toml(&source).expect("manifest parses");
        assert_eq!(
            spec.validate(),
            Err(ValidationError::NotPositive {
                app: "hello".to_owned(),
                field: "idle_timeout_seconds"
            })
        );
    }
}
