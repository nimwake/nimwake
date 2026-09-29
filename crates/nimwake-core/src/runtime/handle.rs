//! Runtime이 외부에 노출하는 process handle.
//!
//! 요구사항: PRD §70 Process Ownership.
//!
//! `tokio::process::Child` 같은 실제 process 객체는 runtime 내부가 유일하게 소유한다.
//! AppManager는 식별자와 제어 경로만 가진다.

use std::net::SocketAddr;

use crate::id::ProcessId;

/// 실행 중인 process를 가리키는 불투명 handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessHandle {
    id: ProcessId,
}

impl ProcessHandle {
    #[must_use]
    pub const fn new(id: ProcessId) -> Self {
        Self { id }
    }

    #[must_use]
    pub const fn id(&self) -> ProcessId {
        self.id
    }
}

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
