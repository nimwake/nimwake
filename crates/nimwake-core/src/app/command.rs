//! AppManager가 처리하는 command / event.
//!
//! 요구사항: PRD §15 Command / Event Model.
//!
//! 설계 규칙:
//! 1. 외부 요청과 내부 lifecycle event가 모두 AppManager를 통해 처리된다.
//! 2. actor 밖에서 발생한 모든 event는 identity를 동반한다. 불일치하면 무시한다.
//! 3. `EnsureRunning`과 `RequestStarted`를 분리하지 않는다. `Acquire` 하나로 원자 처리한다.

use tokio::sync::oneshot;

use crate::error::{AcquireError, AppError, RuntimeError};
use crate::id::{IdleGeneration, LeaseId, OperationId, ProcessId};
use crate::runtime::{ExitStatus, StartedProcess};

use super::lease::RequestLease;
use super::state::AppStatus;

#[derive(Debug)]
pub(crate) enum AppCommand {
    /// RUNNING 확보와 active request 예약을 하나의 transition에서 수행한다. (Invariant 7)
    Acquire {
        reply: oneshot::Sender<Result<RequestLease, AcquireError>>,
    },

    /// `RequestLease`가 drop될 때 전달된다.
    RequestFinished {
        lease_id: LeaseId,
    },

    StartCompleted {
        operation_id: OperationId,
        result: Result<StartedProcess, AppError>,
    },

    StopCompleted {
        operation_id: OperationId,
        result: Result<(), RuntimeError>,
    },

    ProcessExited {
        process_id: ProcessId,
        status: ExitStatus,
    },

    IdleTimeout {
        generation: IdleGeneration,
    },

    /// 관리 목적의 강제 종료 요청.
    Stop {
        reason: StopReason,
    },

    /// 현재 상태 스냅샷 조회. 관측과 테스트에 사용한다.
    Status {
        reply: oneshot::Sender<AppStatus>,
    },

    /// application을 정지시키고 actor loop를 종료한다.
    ///
    /// `ack`는 **process가 실제로 사라진 뒤에** 전달된다.
    /// 여기서 기다리지 않으면 daemon이 먼저 종료해 stop task가 끝나지 못하고 child가 남는다.
    /// (Invariant 10 / PRD §71)
    Shutdown {
        ack: oneshot::Sender<()>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Idle,
    Requested,
    DaemonShutdown,
    /// 메모리가 부족해 내렸다. (PRD §80)
    ///
    /// `Idle`과 구분한다. 운영자가 "스스로 잠든 것"과 "메모리 때문에 내린 것"을
    /// 가를 수 있어야 한다.
    MemoryPressure,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Idle => "idle",
            Self::Requested => "requested",
            Self::DaemonShutdown => "daemon shutdown",
            Self::MemoryPressure => "memory pressure",
        };
        f.write_str(text)
    }
}
