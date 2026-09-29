//! 진행 중인 요청 하나를 나타내는 RAII guard.
//!
//! 요구사항: PRD §26 Acquire와 RequestLease.
//!
//! lease가 살아 있는 동안 해당 application은 idle shutdown 대상이 될 수 없다.
//! drop되면 자동으로 `RequestFinished`를 전달하므로 proxy error / early return /
//! cancellation 경로에서도 counter가 누락되지 않는다.

use tokio::sync::mpsc::{self, error::TrySendError};

use crate::id::LeaseId;
use crate::runtime::Backend;

use super::command::AppCommand;

#[derive(Debug)]
pub struct RequestLease {
    lease_id: LeaseId,
    backend: Backend,
    tx: mpsc::Sender<AppCommand>,
}

impl RequestLease {
    pub(crate) fn new(lease_id: LeaseId, backend: Backend, tx: mpsc::Sender<AppCommand>) -> Self {
        Self {
            lease_id,
            backend,
            tx,
        }
    }

    #[must_use]
    pub fn id(&self) -> LeaseId {
        self.lease_id
    }

    /// proxy 대상 backend.
    #[must_use]
    pub fn backend(&self) -> Backend {
        self.backend
    }
}

impl Drop for RequestLease {
    fn drop(&mut self) {
        let command = AppCommand::RequestFinished {
            lease_id: self.lease_id,
        };

        match self.tx.try_send(command) {
            Ok(()) => {}
            // actor가 이미 종료했다면 반납할 상태도 없다.
            Err(TrySendError::Closed(_)) => {}
            Err(TrySendError::Full(command)) => {
                // mailbox가 가득 찼다고 해서 반납을 버리면 안 된다.
                // 버리는 순간 active_requests가 0으로 내려가지 못해 app이 영원히 잠들지 않는다.
                let tx = self.tx.clone();
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        handle.spawn(async move {
                            let _ = tx.send(command).await;
                        });
                    }
                    Err(_) => {
                        let _ = tx.blocking_send(command);
                    }
                }
            }
        }
    }
}
