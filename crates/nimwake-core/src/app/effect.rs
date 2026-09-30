//! 전이가 **요청하는** 부수효과.
//!
//! 요구사항: PRD §13–§14 Concurrency Model, Invariant 6.
//!
//! 전이 함수는 부수효과를 수행하지 않는다. 무엇을 해야 하는지를 값으로 돌려줄 뿐이다.
//! 실제 수행(`tokio::spawn`, 채널 전송)은 [`Actor`](super::manager)가 전담한다.
//!
//! 이 경계가 있어야
//!
//! - 전이 테스트가 tokio도 `Runtime`도 없이 돌아가고,
//! - "이 전이가 무엇을 예약했는가"를 직접 단언할 수 있으며,
//! - 계측(PRD §72)을 전이 로직이 아니라 수행 지점 한 곳에 붙일 수 있다.

use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::error::AcquireError;
use crate::id::{IdleGeneration, LeaseId, OperationId};
use crate::runtime::{Backend, ProcessHandle};

use super::state::{AppStatus, Waiter};

/// actor가 전이 뒤에 수행할 일.
#[derive(Debug)]
pub(super) enum Effect {
    /// `Runtime::start`를 startup timeout 안에서 수행하고 `StartCompleted`를 보고한다.
    ///
    /// `requested_at`은 기동을 결정한 시각이다. 실제 spawn까지의 대기 구간을 재기 위해
    /// 여기서부터 들고 간다. (PRD §72)
    Start {
        operation_id: OperationId,
        requested_at: Instant,
    },

    /// `Runtime::stop`을 수행한다.
    ///
    /// `operation_id`가 있으면 `StopCompleted`를 보고하고, 없으면 정리만 한다.
    /// 후자는 stale한 startup 결과로 딸려 온 process를 거두는 경로다. (Invariant 8)
    Stop {
        process: ProcessHandle,
        operation_id: Option<OperationId>,
    },

    /// `Runtime::wait`로 process 종료를 감시하고 `ProcessExited`를 보고한다.
    Monitor { process: ProcessHandle },

    /// idle timeout 뒤에 `IdleTimeout { generation }`을 보낸다.
    ///
    /// 발화 시점의 generation이 현재와 다르면 전이가 무시한다. (PRD §29)
    ScheduleIdleTimer { generation: IdleGeneration },

    /// waiter에게 lease를 발급한다.
    ///
    /// 전이는 이미 `lease_id`를 상태에 기록했다. 여기서는 RAII guard를 만들어 건넬 뿐이다.
    /// guard 생성에 필요한 mailbox sender는 actor만 가지고 있다.
    GrantLease {
        waiter: Waiter,
        lease_id: LeaseId,
        backend: Backend,
    },

    /// waiter를 거절한다. overload(`503`)와 기동 실패가 여기로 온다.
    RejectWaiter { waiter: Waiter, error: AcquireError },

    /// 상태 스냅샷을 돌려준다.
    ReportStatus {
        reply: oneshot::Sender<AppStatus>,
        status: AppStatus,
    },

    /// 종료를 기다리던 쪽에 "process가 사라졌다"를 알린다. (Invariant 10)
    ConfirmShutdown { ack: oneshot::Sender<()> },
}
