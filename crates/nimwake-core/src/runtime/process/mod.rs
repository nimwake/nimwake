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
use tokio::time::Instant;
use tracing::debug;

use crate::app::AppSpec;
use crate::error::RuntimeError;
use crate::id::ProcessIdSeq;
use crate::trace::StartTimings;

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
    fn available_memory_mb(&self) -> Option<u64> {
        crate::memory::available_mb()
    }

    /// spawn 후 readiness까지 확인한다. spawn 성공은 readiness가 아니다. (PRD §22)
    ///
    /// startup timeout은 호출자가 적용하며, 그때 이 future는 취소된다.
    /// 취소되면 여기서 만든 `ProcessHandle`이 drop되고 controller가 child를 정리한다.
    async fn start(&self, spec: &AppSpec) -> Result<StartedProcess, RuntimeError> {
        let process_id = self.process_ids.next_id();
        let addr = SocketAddr::from(([127, 0, 0, 1], spec.server.port));

        // spawn보다 먼저 port가 비어 있는지 본다.
        //
        // readiness probe는 "port에 붙을 수 있나"만 묻는다. "내가 띄운 process가
        // 열었나"는 묻지 않는다. 그래서 port가 이미 남의 것이면 이렇게 된다.
        //
        //   1. 자식을 spawn한다
        //   2. 자식은 bind에 실패하지만 아직 살아 있다 (수 ms)
        //   3. probe가 남의 listener에 붙어 성공한다
        //   4. app이 RUNNING이 되고 트래픽이 남의 process로 간다
        //
        // 조기 종료와의 경쟁(아래 select)으로는 막을 수 없다. 자식이 죽기 전에
        // probe가 이기는 것이 정당한 순서이기 때문이다.
        //
        // daemon을 SIGKILL하면 application이 남으므로(destructor가 돌지 않는다)
        // 실제로 일어나는 상황이다. Nimwake는 process를 인수하지 않는다는 계약이
        // 이미 있고, 이 검사가 그 계약을 코드에서도 지킨다. (PRD §71)
        if TcpStream::connect(addr).await.is_ok() {
            return Err(RuntimeError::new(format!(
                "`{}` cannot start: {addr} is already in use by a process Nimwake does not own",
                spec.name
            )));
        }

        // 구간 계측. spawn과 readiness는 보통 자릿수가 다르므로 반드시 나눠 재야 한다. (PRD §42)
        let spawn_started_at = Instant::now();
        let control = controller::spawn(spec, process_id)?;
        let spawned_at = Instant::now();

        let handle = ProcessHandle::new(process_id, control.clone());

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
                timings: StartTimings {
                    spawn_started_at,
                    spawned_at,
                    ready_at: Instant::now(),
                },
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
