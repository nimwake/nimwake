//! Runtime 추상화.
//!
//! 요구사항: PRD §18 Runtime Abstraction.
//!
//! Core는 실행 방식에 종속되지 않는다. runtime adapter가 추가되더라도
//! lifecycle semantics는 변하지 않아야 한다.

mod fake;
mod handle;

use std::time::Duration;

use async_trait::async_trait;

use crate::app::AppSpec;
use crate::error::RuntimeError;

pub use fake::{FakeRuntime, FakeRuntimeConfig};
pub use handle::{Backend, ExitStatus, ProcessHandle, StartedProcess};

/// application을 실행·감시·종료하는 adapter.
///
/// `v0.0.1`은 [`FakeRuntime`]만 제공한다. 실제 process 구현(`ProcessRuntime`)은 `v0.0.2`다.
#[async_trait]
pub trait Runtime: Send + Sync + 'static {
    /// application을 시작하고 readiness까지 확인한 뒤 backend를 돌려준다.
    ///
    /// startup timeout은 호출자(AppManager)가 적용한다.
    async fn start(&self, spec: &AppSpec) -> Result<StartedProcess, RuntimeError>;

    /// process가 종료될 때까지 기다린다.
    ///
    /// AppManager는 이 함수를 monitor task에서 호출해 `ProcessExited`를 보고한다.
    /// 이미 종료된 process에 대해서는 즉시 반환해야 한다.
    async fn wait(&self, handle: &ProcessHandle) -> ExitStatus;

    /// graceful 종료를 시도하고, timeout을 넘기면 강제 종료한다.
    async fn stop(&self, handle: &ProcessHandle, timeout: Duration) -> Result<(), RuntimeError>;
}
