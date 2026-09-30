//! Per-application lifecycle actor — 부수효과 수행 계층.
//!
//! 요구사항: PRD §13 Concurrency Model, §14 AppManager, §16–§17, §68.
//!
//! lifecycle state를 변경할 수 있는 주체는 이 actor 하나뿐이다. (Invariant 2)
//! startup / shutdown / readiness 같은 긴 작업은 actor 밖에서 수행하고,
//! 결과는 identity를 붙인 event로 돌려받는다. (Invariant 6)
//!
//! **무엇을 할지 정하는 것은 이 파일이 아니다.** 전이는 전부
//! [`transition`](super::transition)의 순수 함수가 결정하고, 여기서는 그 결과로 나온
//! [`Effect`]를 실행할 뿐이다. 그래서 이 파일에는 상태 판단이 한 줄도 없다.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
// std::time::Instant는 tokio의 가상 시계(`time::pause`)를 따르지 않는다.
// lifecycle 시간 판단은 모두 tokio 시계를 기준으로 해야 테스트에서 결정적으로 제어된다.
use tokio::time::Instant;
use tracing::debug;

use crate::error::AppError;
use crate::id::{IdleGeneration, OperationId};
use crate::runtime::{ProcessHandle, Runtime};

use super::command::AppCommand;
use super::effect::Effect;
use super::handle::AppHandle;
use super::lease::RequestLease;
use super::spec::AppSpec;
use super::transition::Machine;

/// AppManager mailbox 기본 용량.
///
/// bounded여야 한다. (Invariant 9 / PRD §69)
pub const DEFAULT_MAILBOX_CAPACITY: usize = 128;

/// `AppHandle::acquire`가 mailbox에 명령을 넣기 위해 기다리는 최대 시간.
///
/// 이 시간을 넘기면 overload로 간주한다. (PRD §69)
pub const ACQUIRE_ENQUEUE_TIMEOUT: Duration = Duration::from_millis(50);

/// actor 하나를 띄우고 handle을 돌려준다.
pub fn spawn(spec: Arc<AppSpec>, runtime: Arc<dyn Runtime>) -> AppHandle {
    spawn_with_capacity(spec, runtime, DEFAULT_MAILBOX_CAPACITY)
}

/// mailbox 용량을 지정해 actor를 띄운다. 주로 backpressure 테스트에서 사용한다.
pub fn spawn_with_capacity(
    spec: Arc<AppSpec>,
    runtime: Arc<dyn Runtime>,
    mailbox_capacity: usize,
) -> AppHandle {
    let (tx, rx) = mpsc::channel(mailbox_capacity.max(1));

    let actor = Actor {
        spec: Arc::clone(&spec),
        runtime,
        self_tx: tx.clone(),
        machine: Machine::new(Arc::clone(&spec)),
    };

    tokio::spawn(actor.run(rx));

    AppHandle::new(spec, tx)
}

struct Actor {
    spec: Arc<AppSpec>,
    runtime: Arc<dyn Runtime>,
    /// 자기 자신에게 event를 보내기 위한 sender. lease 발급에도 사용한다.
    self_tx: mpsc::Sender<AppCommand>,
    machine: Machine,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::Receiver<AppCommand>) {
        debug!(app = %self.spec.name, "app manager started");

        while let Some(command) = rx.recv().await {
            let (effects, control) = self.machine.step(command, Instant::now());
            self.apply(effects);

            if control == ControlFlow::Break(()) {
                break;
            }
        }

        let effects = self.machine.teardown();
        self.apply(effects);

        debug!(app = %self.spec.name, "app manager stopped");
    }

    fn apply(&self, effects: Vec<Effect>) {
        for effect in effects {
            self.run_effect(effect);
        }
    }

    fn run_effect(&self, effect: Effect) {
        match effect {
            Effect::Start { operation_id } => self.spawn_start(operation_id),

            Effect::Stop {
                process,
                operation_id,
            } => self.spawn_stop(process, operation_id),

            Effect::Monitor { process } => self.spawn_monitor(process),

            Effect::ScheduleIdleTimer { generation } => self.spawn_idle_timer(generation),

            Effect::GrantLease {
                waiter,
                lease_id,
                backend,
            } => {
                let lease = RequestLease::new(lease_id, backend, self.self_tx.clone());
                // 이미 포기한 호출자라면 lease가 즉시 drop되어 `RequestFinished`가 돌아온다.
                if let Err(Ok(lease)) = waiter.send(Ok(lease)) {
                    drop(lease);
                }
            }

            Effect::RejectWaiter { waiter, error } => {
                let _ = waiter.send(Err(error));
            }

            Effect::ReportStatus { reply, status } => {
                let _ = reply.send(status);
            }

            Effect::ConfirmShutdown { ack } => {
                let _ = ack.send(());
            }
        }
    }

    /// startup timeout은 여기서 건다. 시간이 다하면 `start` future가 **취소**되고,
    /// 그때 `ProcessHandle`이 drop되면서 runtime이 child를 정리한다. (D20 / PRD §71)
    fn spawn_start(&self, operation_id: OperationId) {
        let spec = Arc::clone(&self.spec);
        let runtime = Arc::clone(&self.runtime);
        let tx = self.self_tx.clone();
        let startup_timeout = spec.lifecycle.startup_timeout();

        tokio::spawn(async move {
            let result = match tokio::time::timeout(startup_timeout, runtime.start(&spec)).await {
                Ok(Ok(started)) => Ok(started),
                Ok(Err(error)) => Err(AppError::StartFailed(error.to_string())),
                Err(_elapsed) => Err(AppError::StartupTimeout(startup_timeout)),
            };

            let _ = tx
                .send(AppCommand::StartCompleted {
                    operation_id,
                    result,
                })
                .await;
        });
    }

    /// `operation_id`가 있으면 결과를 actor에 보고하고, 없으면 정리만 한다.
    fn spawn_stop(&self, process: ProcessHandle, operation_id: Option<OperationId>) {
        let runtime = Arc::clone(&self.runtime);
        let tx = self.self_tx.clone();
        let timeout = self.spec.lifecycle.shutdown_timeout();

        tokio::spawn(async move {
            let result = runtime.stop(&process, timeout).await;
            if let Some(operation_id) = operation_id {
                let _ = tx
                    .send(AppCommand::StopCompleted {
                        operation_id,
                        result,
                    })
                    .await;
            }
        });
    }

    /// process 종료를 감시한다. `Child` 소유권은 runtime 안에 있다. (PRD §70)
    fn spawn_monitor(&self, process: ProcessHandle) {
        let runtime = Arc::clone(&self.runtime);
        let tx = self.self_tx.clone();

        tokio::spawn(async move {
            let status = runtime.wait(&process).await;
            let _ = tx
                .send(AppCommand::ProcessExited {
                    process_id: process.id(),
                    status,
                })
                .await;
        });
    }

    fn spawn_idle_timer(&self, generation: IdleGeneration) {
        let tx = self.self_tx.clone();
        let delay = self.spec.lifecycle.idle_timeout();

        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(AppCommand::IdleTimeout { generation }).await;
        });
    }
}
