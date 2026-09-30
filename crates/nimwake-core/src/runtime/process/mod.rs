//! 실제 OS process runtime. (PRD §19)
//!
//! Unix 전용이다. 종료 계약(SIGTERM → timeout → SIGKILL)과 process group 정리는
//! POSIX 신호 위에 서 있고, Nimwake의 Tier 1/2 대상은 모두 Linux 계열이다. (PRD §48)
//!
//! lifecycle semantics는 [`FakeRuntime`](super::FakeRuntime)과 동일해야 한다.
//! adapter가 바뀌어도 state machine은 바뀌지 않는다는 것이 `v0.0.2`의 증명 대상이다.

mod controller;
mod signal;

use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use tokio::net::TcpStream;
use tracing::debug;

use crate::app::AppSpec;
use crate::error::RuntimeError;
use crate::id::ProcessIdSeq;

use super::handle::{Backend, ExitStatus, ProcessHandle, StartedProcess};
use super::Runtime;

/// readiness probe 간격. (PRD §22)
const READINESS_PROBE_INTERVAL: Duration = Duration::from_millis(50);

/// `tokio::process` 기반 runtime.
#[derive(Debug, Default)]
pub struct ProcessRuntime {
    process_ids: ProcessIdSeq,
}

impl ProcessRuntime {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            process_ids: ProcessIdSeq::new(),
        }
    }
}

#[async_trait]
impl Runtime for ProcessRuntime {
    /// spawn 후 readiness까지 확인한다. spawn 성공은 readiness가 아니다. (PRD §22)
    ///
    /// startup timeout은 호출자가 적용하며, 그때 이 future는 취소된다.
    /// 취소되면 여기서 만든 `ProcessHandle`이 drop되고 controller가 child를 정리한다.
    async fn start(&self, spec: &AppSpec) -> Result<StartedProcess, RuntimeError> {
        let process_id = self.process_ids.next_id();
        let control = controller::spawn(spec, process_id)?;
        let handle = ProcessHandle::new(process_id, control.clone());

        let addr = SocketAddr::from(([127, 0, 0, 1], spec.server.port));
        debug!(app = %spec.name, process = %process_id, %addr, "waiting for readiness");

        // readiness와 조기 종료의 경쟁. process가 죽었다면 readiness는 성립할 수 없으므로
        // 둘 다 준비된 순간에는 종료를 택한다. 결과는 어느 쪽이든 결정적이다.
        tokio::select! {
            biased;

            status = wait_for_exit(control.as_ref()) => Err(RuntimeError::new(format!(
                "`{}` exited during startup ({status})",
                spec.name
            ))),

            () = probe_until_ready(addr) => Ok(StartedProcess {
                handle,
                backend: Backend::new(addr),
            }),
        }
    }

    async fn wait(&self, handle: &ProcessHandle) -> ExitStatus {
        handle.control().wait().await
    }

    async fn stop(&self, handle: &ProcessHandle, timeout: Duration) -> Result<(), RuntimeError> {
        handle.control().stop(timeout).await
    }
}

async fn wait_for_exit(control: &controller::ControllerHandle) -> ExitStatus {
    use super::handle::ProcessControl;
    control.wait().await
}

/// loopback TCP connect가 성공할 때까지 재시도한다.
///
/// 상한을 두지 않는다. startup timeout은 AppManager가 이 future를 취소하는 방식으로 적용한다.
async fn probe_until_ready(addr: SocketAddr) {
    loop {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(READINESS_PROBE_INTERVAL).await;
    }
}
