//! Nimwake core lifecycle.
//!
//! 요구사항 원장은 `.docs/prd/`에 있다. 각 모듈 주석이 해당 PRD 섹션을 가리킨다.
//!
//! `v0.0.1 — Lifecycle Foundation`의 범위:
//! OS process 없이 lifecycle state machine과 동시성 모델을 확정한다.
//! HTTP gateway는 `v0.0.3`, 실제 `ProcessRuntime`은 `v0.0.2`다.

pub mod app;
pub mod error;
pub mod id;
pub mod registry;
pub mod runtime;

pub use app::{AppHandle, AppSpec, AppStateName, AppStatus, RequestLease, StopReason};
pub use error::{AcquireError, AppError, ManifestError, RuntimeError, ValidationError};
pub use registry::{AppRegistry, DEFAULT_GATEWAY_PORT};
pub use runtime::Runtime;
