//! Nimwake core lifecycle.
//!
//! 요구사항 원장은 `.docs/prd/`에 있다. 각 모듈 주석이 해당 PRD 섹션을 가리킨다.
//!
//! 현재 범위는 `v0.0.2 — Process Runtime`까지다.
//! lifecycle state machine과 동시성 모델(`v0.0.1`) 위에서 실제 OS process를 다룬다.
//! HTTP gateway와 scale-to-zero 서빙은 `v0.0.3`이다.

pub mod app;
pub mod error;
pub mod id;
pub mod registry;
pub mod runtime;

pub use app::{AppHandle, AppSpec, AppStateName, AppStatus, RequestLease, StopReason};
pub use error::{AcquireError, AppError, ManifestError, RuntimeError, ValidationError};
pub use registry::{AppRegistry, DEFAULT_GATEWAY_PORT};
#[cfg(unix)]
pub use runtime::ProcessRuntime;
pub use runtime::Runtime;
