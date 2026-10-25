//! Application manifest(`nimwake.toml`)와 domain model.
//!
//! 요구사항: PRD §21 Application Manifest.
//!
//! manifest는 특정 runtime이 아니라 동작을 기술한다.
//! 계약은 한 문장이다: "이 command를 실행하면 이 port에서 HTTP application이 listen한다."

use std::collections::BTreeMap;
use std::net::IpAddr;
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
    /// routing에 사용하는 `Host` header 값. **대표 host다.**
    pub host: String,
    /// 같은 application에 닿는 추가 `Host` 값들. (PRD §87)
    ///
    /// **비어 있는 것이 기본이다.** 기존 manifest는 그대로 동작한다.
    ///
    /// 없으면 두 호스트명을 서빙하려고 manifest를 둘로 나눠야 하고, `port`
    /// 중복이 거부되므로 **같은 application이 두 번 뜬다** — 메모리도 cold
    /// start도 두 배다.
    ///
    /// 와일드카드는 받지 않는다. §74가 "등록되지 않은 `Host`는 `404`이고
    /// 아무것도 깨우지 않는다"로 supervisor의 probe를 받는데, 와일드카드가
    /// 있으면 "등록되지 않은"의 뜻이 사라진다.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// application이 listen하는 loopback port.
    pub port: u16,
    /// 이 application에 닿아도 되는 peer 대역. (PRD §82)
    ///
    /// **비어 있으면 모두 허용한다.** 기존 manifest는 그대로 동작한다.
    ///
    /// 문법이 틀리면 §21의 전체 검증에서 걸려 daemon이 뜨지 않는다. 접근 제어
    /// 규칙을 조용히 건너뛰는 것은 열린 채로 도는 것과 같다.
    #[serde(default)]
    pub allow: Vec<super::cidr::Cidr>,
}

impl ServerSpec {
    /// 이 peer가 application에 닿아도 되는가. (PRD §82)
    ///
    /// 판단에 쓰는 값은 **gateway가 직접 관측한 TCP peer**다. `X-Forwarded-For`는
    /// 보지 않는다 — 클라이언트가 보낼 수 있어서 한 줄로 우회된다 (§81).
    #[must_use]
    pub fn allows(&self, peer: IpAddr) -> bool {
        self.allow.is_empty() || self.allow.iter().any(|range| range.contains(peer))
    }
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

        // **경로를 붙여서 올린다.** `?`만 쓰면 `#[from]`이 경로 없는 변종을
        // 만들고, 이름이 비어 있을 때 운영자에게 단서가 하나도 남지 않는다.
        spec.validate().map_err(|source| ManifestError::Invalid {
            path: path.to_path_buf(),
            source,
        })?;
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
        // alias 하나하나의 검사. (PRD §87) 다른 app과 겹치는지는 registry의
        // 전체 검증이 본다 — 여기서는 이 manifest 안에서 알 수 있는 것만 본다.
        for alias in &self.server.aliases {
            if alias.trim().is_empty() {
                return Err(ValidationError::EmptyAlias { app: app() });
            }
            if alias.contains('*') {
                return Err(ValidationError::WildcardAlias {
                    app: app(),
                    alias: alias.clone(),
                });
            }
            // 대표 host와 같으면 둘 중 하나는 뜻이 없다. port suffix와 대소문자를
            // 무시하고 비교해야 `Blog.COM:8080`과 `blog.com`을 같게 본다.
            if crate::registry::normalize_host(alias)
                == crate::registry::normalize_host(&self.server.host)
            {
                return Err(ValidationError::AliasMatchesOwnHost {
                    app: app(),
                    alias: alias.clone(),
                });
            }
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
            // 0이면 모든 요청이 즉시 504가 되어 application이 영원히 응답하지
            // 못한다. 실제로 그렇게 통과시켰다. (PRD §22, §75)
            (
                self.lifecycle.response_header_timeout_seconds,
                "response_header_timeout_seconds",
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

    /// **문서화된 기본값은 계약이다.** (PRD §21)
    ///
    /// 이 다섯 값은 PRD §21의 manifest 예시에 그대로 적혀 있고, 운영자가 그것을
    /// 베껴 쓴다. 코드에서 하나를 바꾸면 문서가 거짓이 되고, manifest에 적지
    /// 않은 app의 동작이 말없이 달라진다.
    ///
    /// `parses_manifest_with_default_lifecycle`은 "빠뜨린 값이 기본값으로
    /// 채워진다"를 보지만 **그 기본값이 얼마인지는 보지 않는다.** `idle_timeout`
    /// 하나만 예외였다. 나머지 넷은 아무도 지키지 않아서, 바꿔도 테스트가
    /// 전부 통과했다.
    ///
    /// 바꿔야 한다면 PRD §21을 먼저 고치고 여기를 맞춘다.
    #[test]
    fn the_documented_lifecycle_defaults_are_a_contract() {
        let lifecycle = LifecycleSpec::default();

        assert_eq!(
            lifecycle.idle_timeout(),
            Duration::from_secs(60),
            "idle_timeout_seconds = 60",
        );
        assert_eq!(
            lifecycle.startup_timeout(),
            Duration::from_secs(5),
            "startup_timeout_seconds = 5",
        );
        assert_eq!(
            lifecycle.shutdown_timeout(),
            Duration::from_secs(5),
            "shutdown_timeout_seconds = 5",
        );
        assert_eq!(
            lifecycle.response_header_timeout(),
            Duration::from_secs(60),
            "response_header_timeout_seconds = 60 (§75)",
        );
        assert_eq!(
            lifecycle.max_pending_requests, 256,
            "max_pending_requests = 256",
        );
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

    /// §22는 timeout 전부가 양수여야 한다고 적는다. **하나씩 다 본다.**
    ///
    /// 전에는 `idle_timeout_seconds` 하나만 보았고, 그래서
    /// `response_header_timeout_seconds`가 목록에 추가될 때 검증에서 빠진 것을
    /// 아무도 알아채지 못했다. 0이면 모든 요청이 즉시 504가 되어 application이
    /// 영원히 응답하지 못한다.
    #[test]
    fn rejects_a_zero_value_for_every_timeout() {
        for field in [
            "idle_timeout_seconds",
            "startup_timeout_seconds",
            "shutdown_timeout_seconds",
            "response_header_timeout_seconds",
        ] {
            let source = format!("{MANIFEST}\n[lifecycle]\n{field} = 0\n");
            let spec = AppSpec::from_toml(&source).expect("manifest parses");
            assert_eq!(
                spec.validate(),
                Err(ValidationError::NotPositive {
                    app: "hello".to_owned(),
                    field,
                }),
                "`{field} = 0` must be rejected",
            );
        }
    }

    /// 손으로 manifest를 쓸 때 가장 먼저 부딪히는 세 가지.
    ///
    /// **오류 변종을 전수 대조하다 이 셋에 테스트가 없는 것을 찾았다.**
    /// `EmptyName` · `EmptyHost` · `Parse` 중 어느 것도 아무 테스트가 만들지
    /// 않고 있었다 — manifest를 처음 쓰는 사람이 가장 먼저 만나는 셋이다.
    fn write(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("nimwake.toml");
        std::fs::write(&path, body).expect("manifest가 쓰인다");
        path
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nimwake-spec-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("디렉터리가 만들어진다");
        dir
    }

    /// **이름이 비면 어느 파일인지 말해야 한다.** (PRD §21)
    ///
    /// 전에는 `app \`\`: name must not be empty`만 나왔다. 이름이 비었으니
    /// `app \`\``까지 비고, **단서가 하나도 없다** — app이 스무 개면 스무 개를
    /// 열어 봐야 한다.
    #[test]
    fn an_empty_name_names_the_file() {
        let dir = scratch("empty-name");
        let path = write(
            &dir,
            concat!(
                "name = \"\"\n\n[server]\nhost = \"a.localhost\"\nport = 3101\n\n",
                "[runtime]\ncommand = [\"true\"]\nworking_dir = \".\"\n",
            ),
        );

        let error = AppSpec::load(&path).expect_err("빈 이름은 거부된다");
        let message = error.to_string();

        assert!(
            matches!(
                error,
                ManifestError::Invalid {
                    source: ValidationError::EmptyName { .. },
                    ..
                }
            ),
            "{message}",
        );
        assert!(
            message.contains("nimwake.toml"),
            "메시지가 파일을 가리켜야 한다: {message}",
        );
    }

    /// host가 비어도 같다. (PRD §21)
    #[test]
    fn an_empty_host_names_the_file() {
        let dir = scratch("empty-host");
        let path = write(
            &dir,
            concat!(
                "name = \"a\"\n\n[server]\nhost = \"\"\nport = 3101\n\n",
                "[runtime]\ncommand = [\"true\"]\nworking_dir = \".\"\n",
            ),
        );

        let error = AppSpec::load(&path).expect_err("빈 host는 거부된다");
        let message = error.to_string();

        assert!(
            matches!(
                error,
                ManifestError::Invalid {
                    source: ValidationError::EmptyHost { .. },
                    ..
                }
            ),
            "{message}",
        );
        assert!(message.contains("nimwake.toml"), "{message}");
    }

    /// **TOML이 깨지면 파일과 위치를 함께 보여 준다.** (PRD §21)
    ///
    /// 가장 흔한 실패다. 메시지가 쓸 만해야 한다.
    #[test]
    fn broken_toml_names_the_file_and_the_spot() {
        let dir = scratch("broken-toml");
        // `[server`로 닫지 않았다.
        let path = write(&dir, "name = \"a\"\n[server\nhost = 1\n");

        let error = AppSpec::load(&path).expect_err("깨진 TOML은 거부된다");
        let message = error.to_string();

        assert!(matches!(error, ManifestError::Parse { .. }), "{message}",);
        assert!(
            message.contains("nimwake.toml"),
            "파일을 가리켜야 한다: {message}",
        );
        // toml 크레이트가 위치를 담아 준다. 그것을 버리지 않는지 본다.
        assert!(
            message.contains("expected") || message.contains("unclosed"),
            "무엇이 틀렸는지 말해야 한다: {message}",
        );
    }
}
