//! Application lifecycle state.
//!
//! 요구사항: PRD §10 Application Lifecycle, §11 상태 모델.
//!
//! 핵심 규칙: startup을 기다리는 waiter는 state 바깥의 별도 구조가 아니라
//! **state가 직접 소유한다.** waiter를 밖에 두면 `FAILED` / `STOPPING` / shutdown
//! 전이에서 waiter를 유실할 수 있다.

use std::collections::HashSet;
use tokio::time::Instant;

use tokio::sync::oneshot;

use crate::error::{AcquireError, AppError};
use crate::id::{IdleGeneration, LeaseId, OperationId, ProcessId};
use crate::runtime::{Backend, ProcessHandle};

use super::lease::RequestLease;

/// startup 완료를 기다리는 호출자.
pub(crate) type Waiter = oneshot::Sender<Result<RequestLease, AcquireError>>;

#[derive(Debug)]
pub(crate) enum AppState {
    Sleeping,

    Starting {
        operation_id: OperationId,
        started_at: Instant,
        waiters: Vec<Waiter>,
    },

    Running(RunningState),

    Stopping {
        operation_id: OperationId,
        process: ProcessHandle,
        waiters: Vec<Waiter>,
    },

    Failed {
        error: AppError,
        failed_at: Instant,
    },
}

#[derive(Debug)]
pub(crate) struct RunningState {
    pub process: ProcessHandle,
    pub backend: Backend,
    pub started_at: Instant,
    /// 살아 있는 lease 집합. `active_requests`는 이 집합의 크기다.
    ///
    /// 단순 카운터 대신 집합을 쓰는 이유는 중복 반납과 이미 교체된 process에 대한
    /// 반납을 무시할 수 있어야 하기 때문이다. (PRD §68 LeaseId)
    pub leases: HashSet<LeaseId>,
    pub last_request_at: Instant,
    pub idle_generation: IdleGeneration,
}

impl AppState {
    pub(crate) fn name(&self) -> AppStateName {
        match self {
            Self::Sleeping => AppStateName::Sleeping,
            Self::Starting { .. } => AppStateName::Starting,
            Self::Running(_) => AppStateName::Running,
            Self::Stopping { .. } => AppStateName::Stopping,
            Self::Failed { .. } => AppStateName::Failed,
        }
    }

    /// 더 이상 정리할 process도, 진행 중인 기동도 없는 상태.
    ///
    /// daemon 종료가 여기 도달해야 actor loop를 끝낼 수 있다. (Invariant 10)
    pub(crate) const fn is_settled(&self) -> bool {
        matches!(self, Self::Sleeping | Self::Failed { .. })
    }

    pub(crate) fn active_requests(&self) -> usize {
        match self {
            Self::Running(running) => running.leases.len(),
            _ => 0,
        }
    }

    pub(crate) fn pending_waiters(&self) -> usize {
        match self {
            Self::Starting { waiters, .. } | Self::Stopping { waiters, .. } => waiters.len(),
            _ => 0,
        }
    }

    pub(crate) fn process_id(&self) -> Option<ProcessId> {
        match self {
            Self::Running(running) => Some(running.process.id()),
            Self::Stopping { process, .. } => Some(process.id()),
            _ => None,
        }
    }

    pub(crate) fn last_error(&self) -> Option<AppError> {
        match self {
            Self::Failed { error, .. } => Some(error.clone()),
            _ => None,
        }
    }

    pub(crate) fn uptime(&self) -> Option<std::time::Duration> {
        match self {
            Self::Running(running) => Some(running.started_at.elapsed()),
            Self::Starting { started_at, .. } => Some(started_at.elapsed()),
            Self::Failed { failed_at, .. } => Some(failed_at.elapsed()),
            _ => None,
        }
    }
}

/// 외부에 노출하는 상태 이름.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppStateName {
    Sleeping,
    Starting,
    Running,
    Stopping,
    Failed,
}

impl std::fmt::Display for AppStateName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Sleeping => "SLEEPING",
            Self::Starting => "STARTING",
            Self::Running => "RUNNING",
            Self::Stopping => "STOPPING",
            Self::Failed => "FAILED",
        };
        f.write_str(text)
    }
}

/// application의 현재 상태 스냅샷.
#[derive(Debug, Clone)]
pub struct AppStatus {
    pub name: String,
    pub state: AppStateName,
    pub active_requests: usize,
    pub pending_waiters: usize,
    pub process_id: Option<ProcessId>,
    pub last_error: Option<AppError>,
    pub since: Option<std::time::Duration>,
}
