//! Per-application lifecycle actor.
//!
//! 요구사항: PRD §13 Concurrency Model, §14 AppManager, §16–§17, §68.
//!
//! lifecycle state를 변경할 수 있는 주체는 이 actor 하나뿐이다. (Invariant 2)
//! startup / shutdown / readiness 같은 긴 작업은 actor 밖에서 수행하고,
//! 결과는 identity를 붙인 event로 돌려받는다. (Invariant 6)

use std::collections::HashSet;
use std::mem;
use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
// std::time::Instant는 tokio의 가상 시계(`time::pause`)를 따르지 않는다.
// lifecycle 시간 판단은 모두 tokio 시계를 기준으로 해야 테스트에서 결정적으로 제어된다.
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::error::{AcquireError, AppError, RuntimeError};
use crate::id::{IdleGeneration, IdleGenerationSeq, LeaseIdSeq, OperationId, OperationIdSeq};
use crate::id::{LeaseId, ProcessId};
use crate::runtime::{ExitStatus, ProcessHandle, Runtime, StartedProcess};

use super::command::{AppCommand, StopReason};
use super::handle::AppHandle;
use super::lease::RequestLease;
use super::spec::AppSpec;
use super::state::{AppState, AppStatus, RunningState, Waiter};

/// AppManager mailbox 기본 용량.
///
/// bounded여야 한다. (Invariant 9 / PRD §69)
pub const DEFAULT_MAILBOX_CAPACITY: usize = 128;

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
        state: AppState::Sleeping,
        operation_ids: OperationIdSeq::new(),
        lease_ids: LeaseIdSeq::new(),
        idle_generations: IdleGenerationSeq::new(),
        shutdown_ack: None,
    };

    tokio::spawn(actor.run(rx));

    AppHandle::new(spec, tx)
}

struct Actor {
    spec: Arc<AppSpec>,
    runtime: Arc<dyn Runtime>,
    /// 자기 자신에게 event를 보내기 위한 sender. lease 발급에도 사용한다.
    self_tx: mpsc::Sender<AppCommand>,
    state: AppState,
    operation_ids: OperationIdSeq,
    lease_ids: LeaseIdSeq,
    idle_generations: IdleGenerationSeq,
    /// daemon 종료를 기다리는 쪽. process가 사라진 뒤에 신호를 보낸다.
    shutdown_ack: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Actor {
    async fn run(mut self, mut rx: mpsc::Receiver<AppCommand>) {
        debug!(app = %self.spec.name, "app manager started");

        while let Some(command) = rx.recv().await {
            if self.handle(command).is_break() {
                break;
            }
        }

        self.on_actor_stopped();
        debug!(app = %self.spec.name, "app manager stopped");
    }

    fn handle(&mut self, command: AppCommand) -> ControlFlow<()> {
        match command {
            AppCommand::Acquire { reply } => self.on_acquire(reply),
            AppCommand::RequestFinished { lease_id } => self.on_request_finished(lease_id),
            AppCommand::StartCompleted {
                operation_id,
                result,
            } => self.on_start_completed(operation_id, result),
            AppCommand::StopCompleted {
                operation_id,
                result,
            } => self.on_stop_completed(operation_id, result),
            AppCommand::ProcessExited { process_id, status } => {
                self.on_process_exited(process_id, status);
            }
            AppCommand::IdleTimeout { generation } => self.on_idle_timeout(generation),
            AppCommand::Stop { reason } => self.on_stop(reason),
            AppCommand::Status { reply } => {
                let _ = reply.send(self.status());
            }
            AppCommand::Shutdown { ack } => self.on_shutdown_requested(ack),
        }

        self.settle_shutdown()
    }

    // ---------------------------------------------------------------- acquire

    /// RUNNING 확보와 active request 예약을 한 transition에서 처리한다. (Invariant 7)
    fn on_acquire(&mut self, reply: Waiter) {
        let state = self.take_state();

        self.state = match state {
            AppState::Sleeping => self.begin_start(vec![reply]),

            AppState::Failed { .. } => self.begin_start(vec![reply]),

            AppState::Starting {
                operation_id,
                started_at,
                mut waiters,
            } => {
                self.enqueue_waiter(&mut waiters, reply);
                AppState::Starting {
                    operation_id,
                    started_at,
                    waiters,
                }
            }

            // shutdown을 취소하지 않는다. stop이 끝난 뒤 다시 기동한다. (PRD §31)
            AppState::Stopping {
                operation_id,
                process,
                mut waiters,
            } => {
                self.enqueue_waiter(&mut waiters, reply);
                AppState::Stopping {
                    operation_id,
                    process,
                    waiters,
                }
            }

            AppState::Running(mut running) => {
                let lease = self.issue_lease(&mut running);
                deliver(reply, lease);
                AppState::Running(running)
            }
        };
    }

    /// pending waiter 상한을 넘으면 적재하지 않고 거절한다. (PRD §69)
    fn enqueue_waiter(&self, waiters: &mut Vec<Waiter>, reply: Waiter) {
        if waiters.len() >= self.spec.lifecycle.max_pending_requests {
            warn!(
                app = %self.spec.name,
                pending = waiters.len(),
                "pending waiter limit reached, rejecting acquire"
            );
            let _ = reply.send(Err(AcquireError::Overloaded));
            return;
        }
        waiters.push(reply);
    }

    fn issue_lease(&self, running: &mut RunningState) -> RequestLease {
        let lease_id = self.lease_ids.next_id();
        running.leases.insert(lease_id);
        running.last_request_at = Instant::now();
        // 대기 중이던 idle timer를 무효화한다. (PRD §29)
        running.idle_generation = self.idle_generations.next_id();

        RequestLease::new(lease_id, running.backend, self.self_tx.clone())
    }

    fn on_request_finished(&mut self, lease_id: LeaseId) {
        let state = self.take_state();

        self.state = match state {
            AppState::Running(mut running) => {
                // 이미 반납된 lease이거나 다른 process 세대의 lease면 무시한다.
                if running.leases.remove(&lease_id) {
                    running.last_request_at = Instant::now();
                    if running.leases.is_empty() {
                        self.schedule_idle_timer(&mut running);
                    }
                }
                AppState::Running(running)
            }
            other => other,
        };
    }

    // ------------------------------------------------------------ transitions

    fn begin_start(&mut self, waiters: Vec<Waiter>) -> AppState {
        let operation_id = self.operation_ids.next_id();
        let spec = Arc::clone(&self.spec);
        let runtime = Arc::clone(&self.runtime);
        let tx = self.self_tx.clone();
        let startup_timeout = spec.lifecycle.startup_timeout();

        info!(app = %spec.name, %operation_id, "starting application");

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

        AppState::Starting {
            operation_id,
            started_at: Instant::now(),
            waiters,
        }
    }

    fn on_start_completed(
        &mut self,
        operation_id: OperationId,
        result: Result<StartedProcess, AppError>,
    ) {
        let state = self.take_state();

        self.state = match state {
            AppState::Starting {
                operation_id: current,
                waiters,
                ..
            } if current == operation_id => match result {
                Ok(started) => self.enter_running(started, waiters),
                Err(error) => {
                    warn!(app = %self.spec.name, %operation_id, %error, "startup failed");
                    for waiter in waiters {
                        let _ = waiter.send(Err(AcquireError::StartFailed(error.clone())));
                    }
                    AppState::Failed {
                        error,
                        failed_at: Instant::now(),
                    }
                }
            },

            other => {
                // stale operation. 그 사이 state가 바뀌었으므로 결과를 적용하지 않는다. (Invariant 8)
                if let Ok(started) = result {
                    warn!(
                        app = %self.spec.name,
                        %operation_id,
                        process = %started.handle.id(),
                        "discarding stale startup result, stopping orphan process"
                    );
                    self.spawn_stop_task(started.handle, None);
                } else {
                    debug!(app = %self.spec.name, %operation_id, "ignoring stale startup failure");
                }
                other
            }
        };
    }

    fn enter_running(&mut self, started: StartedProcess, waiters: Vec<Waiter>) -> AppState {
        info!(
            app = %self.spec.name,
            process = %started.handle.id(),
            backend = %started.backend,
            waiters = waiters.len(),
            "application is running"
        );

        self.spawn_monitor(started.handle.clone());

        let now = Instant::now();
        let mut running = RunningState {
            process: started.handle,
            backend: started.backend,
            started_at: now,
            leases: HashSet::new(),
            last_request_at: now,
            idle_generation: self.idle_generations.next_id(),
        };

        // 모든 waiter가 같은 backend를 받고, 동시에 active request로 예약된다. (PRD §16)
        for waiter in waiters {
            let lease = self.issue_lease(&mut running);
            deliver(waiter, lease);
        }

        if running.leases.is_empty() {
            self.schedule_idle_timer(&mut running);
        }

        AppState::Running(running)
    }

    /// daemon 종료 요청을 기록만 한다. 실제 정리는 [`Self::settle_shutdown`]이 한다.
    ///
    /// ack는 곧바로 보내지 않는다. **process가 사라진 뒤**에 보낸다.
    /// 여기서 기다리지 않으면 daemon이 먼저 종료해 stop task가 끝나지 못하고 child가 남는다.
    fn on_shutdown_requested(&mut self, ack: tokio::sync::oneshot::Sender<()>) {
        self.shutdown_ack = Some(ack);
    }

    /// 매 command 처리 후에 불린다. 종료 중이라면 정리를 한 걸음 진행시킨다.
    ///
    /// STARTING 중에 종료 요청이 오면 기동이 끝나기를 기다렸다가 곧바로 정지시킨다.
    /// 여기서 끊어 버리면 방금 뜬 process를 아무도 거두지 않는다.
    fn settle_shutdown(&mut self) -> ControlFlow<()> {
        if self.shutdown_ack.is_none() {
            return ControlFlow::Continue(());
        }

        if matches!(self.state, AppState::Running(_)) {
            let AppState::Running(running) = self.take_state() else {
                unreachable!("just checked the state is RUNNING")
            };
            self.state = self.begin_stop(running.process, Vec::new(), StopReason::DaemonShutdown);
        }

        if !self.state.is_settled() {
            return ControlFlow::Continue(());
        }

        if let Some(ack) = self.shutdown_ack.take() {
            let _ = ack.send(());
        }
        ControlFlow::Break(())
    }

    fn begin_stop(
        &mut self,
        process: ProcessHandle,
        waiters: Vec<Waiter>,
        reason: StopReason,
    ) -> AppState {
        let operation_id = self.operation_ids.next_id();

        info!(
            app = %self.spec.name,
            %operation_id,
            process = %process.id(),
            %reason,
            "stopping application"
        );

        self.spawn_stop_task(process.clone(), Some(operation_id));

        AppState::Stopping {
            operation_id,
            process,
            waiters,
        }
    }

    fn on_stop_completed(&mut self, operation_id: OperationId, result: Result<(), RuntimeError>) {
        if let Err(error) = &result {
            // stop 실패는 상태 전이를 막지 않는다. runtime이 강제 종료까지 책임진다. (PRD §30)
            warn!(app = %self.spec.name, %operation_id, %error, "stop reported an error");
        }

        let state = self.take_state();

        self.state = match state {
            AppState::Stopping {
                operation_id: current,
                waiters,
                ..
            } if current == operation_id => self.finish_stop(waiters),
            other => other,
        };
    }

    fn on_process_exited(&mut self, process_id: ProcessId, status: ExitStatus) {
        let state = self.take_state();

        self.state = match state {
            AppState::Running(running) if running.process.id() == process_id => {
                warn!(
                    app = %self.spec.name,
                    process = %process_id,
                    %status,
                    leases = running.leases.len(),
                    "application process exited unexpectedly"
                );
                // 살아 있던 lease는 모두 무효가 된다. (PRD §32)
                AppState::Failed {
                    error: AppError::ProcessExited(status.to_string()),
                    failed_at: Instant::now(),
                }
            }

            // STOPPING 중의 exit은 정상 종료 경로다. (PRD §32)
            AppState::Stopping {
                process, waiters, ..
            } if process.id() == process_id => self.finish_stop(waiters),

            other => {
                debug!(
                    app = %self.spec.name,
                    process = %process_id,
                    state = %other.name(),
                    "ignoring stale process exit"
                );
                other
            }
        };
    }

    fn finish_stop(&mut self, waiters: Vec<Waiter>) -> AppState {
        // daemon이 내려가는 중이라면 재기동하지 않는다. 기다리던 요청은 실패시킨다.
        if self.shutdown_ack.is_some() {
            for waiter in waiters {
                let _ = waiter.send(Err(AcquireError::Unavailable));
            }
            return AppState::Sleeping;
        }

        if waiters.is_empty() {
            AppState::Sleeping
        } else {
            // stop을 기다리던 요청이 있으면 바로 다시 기동한다. waiter는 유실하지 않는다.
            self.begin_start(waiters)
        }
    }

    fn on_idle_timeout(&mut self, generation: IdleGeneration) {
        let state = self.take_state();
        let idle_timeout = self.spec.lifecycle.idle_timeout();

        self.state = match state {
            AppState::Running(running)
                if running.idle_generation == generation
                    && running.leases.is_empty()
                    && running.last_request_at.elapsed() >= idle_timeout =>
            {
                self.begin_stop(running.process, Vec::new(), StopReason::Idle)
            }

            other => {
                debug!(
                    app = %self.spec.name,
                    %generation,
                    state = %other.name(),
                    "ignoring stale idle timeout"
                );
                other
            }
        };
    }

    fn on_stop(&mut self, reason: StopReason) {
        let state = self.take_state();

        self.state = match state {
            AppState::Running(running) => self.begin_stop(running.process, Vec::new(), reason),
            other => other,
        };
    }

    /// actor loop가 끝난 뒤의 마지막 정리.
    ///
    /// mailbox가 닫혀 loop가 끝나는 경로(`Shutdown` 없이)도 있으므로 best-effort로 남긴다.
    fn on_actor_stopped(&mut self) {
        let state = self.take_state();

        match state {
            AppState::Starting { waiters, .. } | AppState::Stopping { waiters, .. } => {
                for waiter in waiters {
                    let _ = waiter.send(Err(AcquireError::Unavailable));
                }
            }
            AppState::Running(running) => {
                self.spawn_stop_task(running.process, None);
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------- side effects

    fn schedule_idle_timer(&self, running: &mut RunningState) {
        let generation = self.idle_generations.next_id();
        running.idle_generation = generation;

        let tx = self.self_tx.clone();
        let delay = self.spec.lifecycle.idle_timeout();

        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(AppCommand::IdleTimeout { generation }).await;
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

    /// `operation_id`가 있으면 결과를 actor에 보고하고, 없으면 정리만 한다.
    fn spawn_stop_task(&self, process: ProcessHandle, operation_id: Option<OperationId>) {
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

    // ---------------------------------------------------------------- helpers

    fn take_state(&mut self) -> AppState {
        mem::replace(&mut self.state, AppState::Sleeping)
    }

    fn status(&self) -> AppStatus {
        AppStatus {
            name: self.spec.name.clone(),
            state: self.state.name(),
            active_requests: self.state.active_requests(),
            pending_waiters: self.state.pending_waiters(),
            process_id: self.state.process_id(),
            last_error: self.state.last_error(),
            since: self.state.uptime(),
        }
    }
}

/// waiter에게 lease를 전달한다.
///
/// 이미 포기한 호출자라면 lease가 즉시 drop되어 `RequestFinished`가 돌아온다.
fn deliver(reply: oneshot::Sender<Result<RequestLease, AcquireError>>, lease: RequestLease) {
    if let Err(Ok(lease)) = reply.send(Ok(lease)) {
        drop(lease);
    }
}

/// `AppHandle::acquire`가 mailbox에 명령을 넣기 위해 기다리는 최대 시간.
///
/// 이 시간을 넘기면 overload로 간주한다. (PRD §69)
pub const ACQUIRE_ENQUEUE_TIMEOUT: Duration = Duration::from_millis(50);

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
