//! Runtime이 외부에 노출하는 process handle.
//!
//! 요구사항: PRD §70 Process Ownership.
//!
//! `tokio::process::Child` 같은 실제 process 객체는 runtime 내부가 유일하게 소유한다.
//! AppManager는 식별자와 제어 경로만 가진다.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::error::RuntimeError;
use crate::id::ProcessId;
use crate::trace::StartTimings;

/// handle 뒤에 있는 실제 process를 제어하는 경로. (PRD §70의 `control`)
///
/// runtime adapter마다 구현이 다르다. `ProcessRuntime`에서는 `Child`를 소유한
/// controller task로 가는 채널이고, `FakeRuntime`에서는 in-memory 상태다.
///
/// AppManager가 `Child`를 직접 만지지 못하게 하는 경계다.
#[async_trait]
pub trait ProcessControl: Send + Sync + std::fmt::Debug {
    /// process가 종료될 때까지 기다린다. 이미 종료됐으면 즉시 반환한다.
    async fn wait(&self) -> ExitStatus;

    /// graceful 종료를 시도하고, timeout을 넘기면 강제 종료한다.
    async fn stop(&self, timeout: Duration) -> Result<(), RuntimeError>;
}

/// 실행 중인 process를 가리키는 불투명 handle.
///
/// **이 handle의 생존이 process의 생존 조건이다.** 모든 복제본이 drop되면
/// controller는 대상 process를 정리한다. startup timeout으로 `Runtime::start`가
/// 취소돼 handle이 버려지는 경우에도 child가 남지 않는 이유다. (Invariant 10 / PRD §71)
#[derive(Debug, Clone)]
pub struct ProcessHandle {
    id: ProcessId,
    /// OS의 pid. 없을 수 있다 — `FakeRuntime`에는 실제 process가 없다.
    ///
    /// [`ProcessId`]와 **다른 것이다.** 그쪽은 stale event 판정(Invariant 8)을 위한
    /// 단조 식별자이고 OS와 무관하다. 이쪽은 운영자가 `ps`·`/proc`과 맞춰 보는 값이다.
    /// 상태 보고(§79)가 이것을 쓴다.
    os_pid: Option<u32>,
    control: Arc<dyn ProcessControl>,
}

impl ProcessHandle {
    #[must_use]
    pub fn new(id: ProcessId, os_pid: Option<u32>, control: Arc<dyn ProcessControl>) -> Self {
        Self {
            id,
            os_pid,
            control,
        }
    }

    #[must_use]
    pub const fn id(&self) -> ProcessId {
        self.id
    }

    /// OS의 pid. 운영자가 `ps`·`/proc`과 맞춰 보는 값이다. (PRD §79)
    #[must_use]
    pub const fn os_pid(&self) -> Option<u32> {
        self.os_pid
    }

    #[must_use]
    pub fn control(&self) -> &Arc<dyn ProcessControl> {
        &self.control
    }
}

/// process의 동일성은 [`ProcessId`]가 결정한다.
///
/// stale event 판정(Invariant 8)이 보는 것도 id 하나뿐이다.
/// control 경로는 같은 process를 가리키는 복제본끼리 다를 수 있으므로 비교에 넣지 않는다.
impl PartialEq for ProcessHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ProcessHandle {}

/// proxy가 요청을 전달할 대상 주소.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backend {
    addr: SocketAddr,
}

impl Backend {
    #[must_use]
    pub const fn new(addr: SocketAddr) -> Self {
        Self { addr }
    }

    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }
}

impl std::fmt::Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.addr)
    }
}

/// `Runtime::start`가 성공했을 때 돌려주는 값.
///
/// readiness까지 끝난 상태여야 한다. spawn 성공은 readiness를 의미하지 않는다. (PRD §22)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartedProcess {
    pub handle: ProcessHandle,
    pub backend: Backend,
    /// 기동 구간 계측. cold start를 단일 숫자가 아니라 구간별로 보기 위한 것이다. (PRD §72)
    pub timings: StartTimings,
}

/// process 종료 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus {
    pub code: Option<i32>,
}

impl ExitStatus {
    #[must_use]
    pub const fn from_code(code: i32) -> Self {
        Self { code: Some(code) }
    }

    #[must_use]
    pub const fn unknown() -> Self {
        Self { code: None }
    }

    #[must_use]
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

impl std::fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(code) => write!(f, "exit code {code}"),
            None => write!(f, "terminated by signal"),
        }
    }
}
