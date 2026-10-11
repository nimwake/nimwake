//! Nimwake core lifecycle.
//!
//! 요구사항 원장은 `.docs/prd/`에 있다. 각 모듈 주석이 해당 PRD 섹션을 가리킨다.
//!
//! 현재 범위는 `v0.0.3 — HTTP + Scale-to-Zero`까지다.
//!
//! ```text
//! request → wake → serve → idle → sleep
//! ```
//!
//! lifecycle state machine(`v0.0.1`) 위에 실제 OS process(`v0.0.2`)와 gateway(`v0.0.3`)가 올라가 있다.

pub mod app;
pub mod error;
pub mod gateway;
pub mod id;
pub mod limits;
mod memory;
pub mod registry;
pub mod runtime;
pub mod trace;

pub use app::{AppHandle, AppSpec, AppStateName, AppStatus, RequestLease, StopReason};
pub use error::{AcquireError, AppError, ManifestError, RuntimeError, ValidationError};
pub use gateway::Gateway;
pub use limits::Limits;
pub use registry::{AppRegistry, DEFAULT_GATEWAY_PORT};
#[cfg(unix)]
pub use runtime::ProcessRuntime;
pub use runtime::Runtime;
pub use trace::{RequestTrace, StartTimings, StartupTrace};
