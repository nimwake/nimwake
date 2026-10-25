//! Core 에러 타입.

use std::path::PathBuf;
use std::time::Duration;

use thiserror::Error;

/// application lifecycle이 실패한 이유.
///
/// startup을 기다리던 모든 waiter에게 복제되어 전달되므로 `Clone`이어야 한다.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AppError {
    #[error("startup timed out after {0:?}")]
    StartupTimeout(Duration),

    #[error("runtime failed to start the application: {0}")]
    StartFailed(String),

    #[error("application process exited unexpectedly ({0})")]
    ProcessExited(String),
}

/// `AppHandle::acquire` 실패 이유.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AcquireError {
    /// mailbox 또는 pending waiter 상한을 초과했다. Gateway는 `503`으로 응답한다.
    ///
    /// 요구사항: PRD §69 Backpressure와 Overload.
    #[error("application is overloaded")]
    Overloaded,

    #[error("application failed to start: {0}")]
    StartFailed(#[source] AppError),

    /// crash loop로 판정해 재시도를 미루고 있다. Gateway는 `503`과 `Retry-After`로 응답한다.
    ///
    /// `StartFailed`와 구분하는 이유: 그쪽은 시도하고 실패했다는 뜻이고,
    /// 이쪽은 **시도하지 않았다**는 뜻이다.
    ///
    /// 요구사항: PRD §77 Crash-Loop Backoff.
    #[error("application is backing off after repeated failures")]
    BackingOff { retry_after: std::time::Duration },

    /// 가용 메모리가 임계값 아래여서 깨우지 않았다. Gateway는 `503`으로 응답한다.
    ///
    /// `Retry-After`를 붙이지 않는다. 메모리가 언제 풀릴지 **모른다.**
    ///
    /// 요구사항: PRD §78 Memory Admission Control.
    #[error(
        "not enough free memory to start: {available_mb}MiB available, {required_mb}MiB required"
    )]
    InsufficientMemory { available_mb: u64, required_mb: u64 },

    /// AppManager가 이미 종료되어 요청을 받을 수 없다.
    #[error("application manager is unavailable")]
    Unavailable,
}

/// runtime adapter가 보고하는 실패.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct RuntimeError(pub String);

impl RuntimeError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// manifest 파일 처리 실패.
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("failed to read manifest `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse manifest `{path}`: {source}")]
    Parse {
        path: PathBuf,
        // toml 에러는 그 자체로 커서, box하지 않으면 모든 Result가 함께 무거워진다.
        #[source]
        source: Box<toml::de::Error>,
    },

    /// 단일 app 검증 실패. **경로를 함께 담는다.**
    ///
    /// 전에는 `#[error(transparent)]`여서 경로가 사라졌다. `name = ""`인 manifest
    /// 하나가 있으면 운영자가 보는 것이 이것뿐이었다.
    ///
    /// ```text
    /// failed to load manifests from `/srv/apps`: app ``: name must not be empty
    /// ```
    ///
    /// **어느 파일인지 말하지 않는다.** app이 스무 개면 스무 개를 열어 봐야 한다.
    /// 이름이 비어 있을 때는 `app \`\``까지 비므로 단서가 아예 없다.
    #[error("invalid manifest `{path}`: {source}")]
    Invalid {
        path: PathBuf,
        #[source]
        source: ValidationError,
    },
}

/// manifest / registry 검증 실패.
///
/// 요구사항: PRD §21 Manifest Validation.
/// 검증은 daemon 기동 단계에서 수행하며, 하나라도 위반하면 기동을 거부한다.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("app `{app}`: name must not be empty")]
    EmptyName { app: String },

    #[error("app `{app}`: host must not be empty")]
    EmptyHost { app: String },

    #[error("app `{app}`: an alias must not be empty")]
    EmptyAlias { app: String },

    #[error(
        "app `{app}`: alias `{alias}` is the same as its own host — one of the two says nothing"
    )]
    AliasMatchesOwnHost { app: String, alias: String },

    #[error("app `{app}`: alias `{alias}` contains `*` — wildcards are not accepted (PRD §87)")]
    WildcardAlias { app: String, alias: String },

    #[error("app `{app}`: command must not be empty")]
    EmptyCommand { app: String },

    #[error("app `{app}`: working_dir `{path}` does not exist")]
    MissingWorkingDir { app: String, path: PathBuf },

    #[error("app `{app}`: {field} must be greater than zero")]
    NotPositive { app: String, field: &'static str },

    #[error("duplicate app name `{name}`")]
    DuplicateName { name: String },

    #[error("duplicate host `{host}` used by `{first}` and `{second}`")]
    DuplicateHost {
        host: String,
        first: String,
        second: String,
    },

    #[error("duplicate port {port} used by `{first}` and `{second}`")]
    DuplicatePort {
        port: u16,
        first: String,
        second: String,
    },

    #[error("app `{app}` uses the gateway port {port}")]
    GatewayPortConflict { app: String, port: u16 },
}
