//! Gateway가 사용하는 application handle.
//!
//! 요구사항: PRD §26 Acquire와 RequestLease, Invariant 3 Gateway Isolation.
//!
//! Gateway는 application의 상태를 검사하지 않는다. RUNNING이든 SLEEPING이든
//! 동일하게 `acquire()`를 호출하고, wake 여부는 AppManager가 판단한다.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::error::AcquireError;

use super::command::{AppCommand, StopReason};
use super::lease::RequestLease;
use super::manager::ACQUIRE_ENQUEUE_TIMEOUT;
use super::spec::AppSpec;
use super::state::AppStatus;

/// shutdown ack를 기다리는 여유분. runtime의 SIGTERM→SIGKILL 경로보다 넉넉해야 한다.
const SHUTDOWN_ACK_MARGIN: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct AppHandle {
    spec: Arc<AppSpec>,
    tx: mpsc::Sender<AppCommand>,
}

impl AppHandle {
    pub(crate) fn new(spec: Arc<AppSpec>, tx: mpsc::Sender<AppCommand>) -> Self {
        Self { spec, tx }
    }

    #[must_use]
    pub fn spec(&self) -> &Arc<AppSpec> {
        &self.spec
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.spec.name
    }

    #[must_use]
    pub fn host(&self) -> &str {
        &self.spec.server.host
    }

    /// application이 RUNNING임을 보장하고 진행 중인 요청 하나를 예약한다.
    ///
    /// 두 연산은 AppManager의 한 transition에서 원자적으로 수행된다. (Invariant 7)
    /// 반환된 lease가 살아 있는 동안 application은 idle shutdown되지 않는다.
    pub async fn acquire(&self) -> Result<RequestLease, AcquireError> {
        let (reply, response) = oneshot::channel();

        match tokio::time::timeout(
            ACQUIRE_ENQUEUE_TIMEOUT,
            self.tx.send(AppCommand::Acquire { reply }),
        )
        .await
        {
            // mailbox가 계속 가득 차 있으면 overload다. Gateway는 503으로 응답한다.
            Err(_elapsed) => return Err(AcquireError::Overloaded),
            Ok(Err(_closed)) => return Err(AcquireError::Unavailable),
            Ok(Ok(())) => {}
        }

        response.await.unwrap_or(Err(AcquireError::Unavailable))
    }

    /// 현재 상태 스냅샷을 조회한다.
    pub async fn status(&self) -> Result<AppStatus, AcquireError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(AppCommand::Status { reply })
            .await
            .map_err(|_| AcquireError::Unavailable)?;
        response.await.map_err(|_| AcquireError::Unavailable)
    }

    /// 관리 목적의 강제 종료 요청.
    pub async fn stop(&self, reason: StopReason) -> Result<(), AcquireError> {
        self.tx
            .send(AppCommand::Stop { reason })
            .await
            .map_err(|_| AcquireError::Unavailable)
    }

    /// application을 정지시키고 actor loop를 종료한다.
    ///
    /// **process가 실제로 사라질 때까지 기다린다.** daemon 종료 경로에서 이것을 기다리지 않으면
    /// stop task가 끝나기 전에 daemon이 죽어 child가 남는다. (Invariant 10 / PRD §71)
    ///
    /// runtime이 shutdown timeout 뒤 SIGKILL까지 책임지므로 ack는 반드시 온다.
    /// 그럼에도 상한을 두는 것은 actor가 고장났을 때 daemon이 영원히 멈추지 않게 하기 위해서다.
    pub async fn shutdown(&self) {
        let (ack, acked) = oneshot::channel();

        if self.tx.send(AppCommand::Shutdown { ack }).await.is_err() {
            // actor가 이미 끝났다.
            return;
        }

        let deadline = self.spec.lifecycle.shutdown_timeout() * 2 + SHUTDOWN_ACK_MARGIN;
        if tokio::time::timeout(deadline, acked).await.is_err() {
            tracing::warn!(
                app = %self.spec.name,
                ?deadline,
                "application did not confirm shutdown in time",
            );
        }
    }
}
