//! Lifecycle 상태 전이 — 순수 함수 계층.
//!
//! 요구사항: PRD §10–§11 상태 모델, §15 Command/Event Model, §68 Lifecycle Identity.
//!
//! 이 파일의 모든 전이 함수는 다음 형태다.
//!
//! ```text
//! (상태, 이벤트, 문맥) -> (새 상태, 수행할 Effect 목록)
//! ```
//!
//! 상태를 **값으로 받아 값으로 돌려준다.** 제자리에서 고치지 않는다.
//! `tokio`도, [`Runtime`](crate::runtime::Runtime)도, 채널도 여기서는 보이지 않는다.
//! 시각조차 [`Ctx::now`]로 주입받으므로, 시계를 조작하지 않고도 idle 판정을 검증할 수 있다.
//!
//! 덕분에 전이 테스트는 async runtime 없이, fake 없이, 이벤트 루프를 굴리지 않고
//! "이 상태에서 이 이벤트가 오면 저 상태가 되고 이것들을 예약한다"를 그대로 쓴다.
//!
//! ## 구성
//!
//! ```text
//! on_*        AppCommand 하나에 대응하는 전이. 상태를 값으로 받아 값으로 돌려준다.
//! begin_*     긴 작업을 시작하는 전이. Effect를 하나 남긴다.
//! finish_*    긴 작업이 끝났을 때의 전이.
//! Machine     상태를 전이 함수들 사이로 흘려보내는 껍데기.
//! ```
//!
//! ## 두 가지 예외
//!
//! 1. **식별자 발급**([`Ids`]). 새 `OperationId` / `LeaseId` / `IdleGeneration`은
//!    전이가 상태에 기록해야 하므로 밖에서 만들어 올 수 없다. 0에서 시작해 단조 증가하므로
//!    같은 순서의 이벤트에 대해 같은 값이 나온다. 테스트는 여전히 결정적이다.
//! 2. **`tracing`**. 관측 전용이고 프로그램이 볼 수 있는 상태를 바꾸지 않는다.
//!    stale 이벤트를 왜 버렸는지는 전이만 알고 있어서, 이것까지 밖으로 빼면 진단이 사라진다.

use std::collections::HashSet;
use std::ops::ControlFlow;

use tokio::sync::oneshot;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::error::{AcquireError, AppError, RuntimeError};
use crate::id::{
    IdleGeneration, IdleGenerationSeq, LeaseId, LeaseIdSeq, OperationId, OperationIdSeq, ProcessId,
};
use crate::runtime::{ExitStatus, ProcessHandle, StartedProcess};

use crate::limits::Limits;

use super::backoff::{self, FailureHistory};
use super::command::{AppCommand, StopReason};
use super::effect::Effect;
use super::spec::AppSpec;
use super::state::{AppState, AppStatus, RunningState, Waiter};

/// 전이가 상태에 기록할 새 식별자를 발급한다. (PRD §68)
#[derive(Debug, Default)]
pub(super) struct Ids {
    operations: OperationIdSeq,
    leases: LeaseIdSeq,
    idle_generations: IdleGenerationSeq,
}

impl Ids {
    pub(super) const fn new() -> Self {
        Self {
            operations: OperationIdSeq::new(),
            leases: LeaseIdSeq::new(),
            idle_generations: IdleGenerationSeq::new(),
        }
    }
}

/// 전이가 참조하는 읽기 전용 문맥.
/// 전이가 필요로 하는 외부 입력.
///
/// 시계(`now`)처럼 **주입받는다.** 전이 함수가 직접 `/proc`을 읽으면 같은
/// (상태, 명령)이 다른 결과를 낼 수 있고 테스트가 값을 정할 수 없다. (PRD §18)
///
/// 명령에 따라 비어 있을 수 있다. 필요하지 않은 경로에서 읽지 않기 때문이다.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Inputs {
    /// 가용 메모리(MiB). 입장 제어가 필요한 명령에서만 채운다. (PRD §78)
    pub available_memory_mb: Option<u64>,
}

pub(super) struct Ctx<'a> {
    pub spec: &'a AppSpec,
    pub ids: &'a Ids,
    /// daemon 수준 한계값. (PRD §78)
    pub limits: Limits,
    /// 이 전이에 주어진 외부 입력.
    pub inputs: Inputs,
    /// 이 전이가 일어난 시각. 주입받으므로 idle 판정을 시계 조작 없이 검증할 수 있다.
    pub now: Instant,
    /// daemon이 내려가는 중인가. 참이면 stop 완료 후 재기동하지 않는다. (Invariant 10)
    pub shutting_down: bool,
}

/// 전이의 결과.
#[derive(Debug)]
pub(super) struct Step {
    pub state: AppState,
    pub effects: Vec<Effect>,
}

impl Step {
    fn new(state: AppState) -> Self {
        Self {
            state,
            effects: Vec::new(),
        }
    }

    fn with(state: AppState, effects: Vec<Effect>) -> Self {
        Self { state, effects }
    }
}

/// RUNNING 확보와 active request 예약을 한 전이 안에서 처리한다. (Invariant 7)
///
/// 둘을 나누면 그 사이에 idle shutdown이 끼어들 수 있다. (PRD §26)
pub(super) fn on_acquire(state: AppState, waiter: Waiter, ctx: &Ctx<'_>) -> Step {
    match state {
        AppState::Sleeping => match admit(ctx) {
            Err(error) => Step::with(
                AppState::Sleeping,
                vec![Effect::RejectWaiter { waiter, error }],
            ),
            Ok(()) => begin_start(vec![waiter], FailureHistory::default(), ctx),
        },

        // crash loop라면 지금 재시도하지 않는다. (PRD §77)
        AppState::Failed {
            error: error_state,
            failed_at,
            failures,
        } => match backoff::remaining_backoff(&failures, failed_at, ctx.now) {
            Some(retry_after) => {
                warn!(
                    app = %ctx.spec.name,
                    retry_after_ms = retry_after.as_millis(),
                    "request rejected while backing off",
                );
                Step::with(
                    AppState::Failed {
                        error: error_state,
                        failed_at,
                        failures,
                    },
                    vec![Effect::RejectWaiter {
                        waiter,
                        error: AcquireError::BackingOff { retry_after },
                    }],
                )
            }
            None => match admit(ctx) {
                Err(error) => Step::with(
                    AppState::Failed {
                        error: error_state,
                        failed_at,
                        failures,
                    },
                    vec![Effect::RejectWaiter { waiter, error }],
                ),
                Ok(()) => begin_start(vec![waiter], failures, ctx),
            },
        },

        AppState::Starting {
            operation_id,
            started_at,
            waiters,
            failures,
        } => {
            let (waiters, effects) = enqueue(waiters, waiter, ctx);
            Step::with(
                AppState::Starting {
                    operation_id,
                    started_at,
                    waiters,
                    failures,
                },
                effects,
            )
        }

        // shutdown을 취소하지 않는다. stop이 끝난 뒤 다시 기동한다. (PRD §31)
        AppState::Stopping {
            operation_id,
            process,
            waiters,
        } => {
            let (waiters, effects) = enqueue(waiters, waiter, ctx);
            Step::with(
                AppState::Stopping {
                    operation_id,
                    process,
                    waiters,
                },
                effects,
            )
        }

        AppState::Running(running) => {
            let (running, effect) = grant(running, waiter, ctx);
            Step::with(AppState::Running(running), vec![effect])
        }
    }
}

/// pending waiter 상한을 넘으면 적재하지 않고 거절한다. (PRD §69)
fn enqueue(mut waiters: Vec<Waiter>, waiter: Waiter, ctx: &Ctx<'_>) -> (Vec<Waiter>, Vec<Effect>) {
    if waiters.len() >= ctx.spec.lifecycle.max_pending_requests {
        warn!(
            app = %ctx.spec.name,
            pending = waiters.len(),
            "pending waiter limit reached, rejecting acquire",
        );
        return (
            waiters,
            vec![Effect::RejectWaiter {
                waiter,
                error: AcquireError::Overloaded,
            }],
        );
    }

    waiters.push(waiter);
    (waiters, Vec::new())
}

/// lease를 하나 발급해 상태에 기록하고, 전달은 effect에 맡긴다.
fn grant(mut running: RunningState, waiter: Waiter, ctx: &Ctx<'_>) -> (RunningState, Effect) {
    let lease_id = ctx.ids.leases.next_id();
    running.leases.insert(lease_id);
    running.last_request_at = ctx.now;
    // 대기 중이던 idle timer를 무효화한다. (PRD §29)
    running.idle_generation = ctx.ids.idle_generations.next_id();
    let backend = running.backend;

    (
        running,
        Effect::GrantLease {
            waiter,
            lease_id,
            backend,
        },
    )
}

/// lease 반납. `active_requests`는 이 경로로만 줄어든다. (PRD §25)
pub(super) fn on_request_finished(state: AppState, lease_id: LeaseId, ctx: &Ctx<'_>) -> Step {
    let AppState::Running(mut running) = state else {
        return Step::new(state);
    };

    // 이미 반납된 lease이거나 다른 process 세대의 lease면 무시한다. (D16)
    if !running.leases.remove(&lease_id) {
        return Step::new(AppState::Running(running));
    }

    running.last_request_at = ctx.now;
    if !running.leases.is_empty() {
        return Step::new(AppState::Running(running));
    }

    let (running, effect) = arm_idle_timer(running, ctx);
    Step::with(AppState::Running(running), vec![effect])
}

/// 잠든 application을 깨울 수 있는가. (PRD §78)
///
/// **돌고 있는 것은 건드리지 않는다.** 이 검사는 새로 깨우는 경로에만 있다.
/// stop 직후의 재기동(§31)에도 적용하지 않는다 — 방금 그 application이 쓰던
/// 메모리가 풀렸고, stop을 기다린 요청을 거절하는 것이 가장 나쁜 결과다.
///
/// 가용량을 읽을 수 없으면 **통과시킨다.** 판독 실패로 서비스를 멈추지 않는다.
fn admit(ctx: &Ctx<'_>) -> Result<(), AcquireError> {
    // 임계값 0을 따로 걸러내지 않는다. `u64 >= 0`은 늘 참이므로 아래 비교가 통과한다.
    // **꺼져 있을 때 `/proc`을 읽지 않는 것**이 중요하고 그 판단은 actor가 한다.
    // 여기서 한 번 더 거르면 어떤 테스트도 구분하지 못하는 가지가 생긴다.
    let required_mb = ctx.limits.min_free_memory_mb;

    let Some(available_mb) = ctx.inputs.available_memory_mb else {
        return Ok(());
    };

    if available_mb >= required_mb {
        return Ok(());
    }

    warn!(
        app = %ctx.spec.name,
        available_mb,
        required_mb,
        "refusing to wake: not enough free memory",
    );
    Err(AcquireError::InsufficientMemory {
        available_mb,
        required_mb,
    })
}

fn begin_start(waiters: Vec<Waiter>, failures: FailureHistory, ctx: &Ctx<'_>) -> Step {
    let operation_id = ctx.ids.operations.next_id();
    info!(app = %ctx.spec.name, %operation_id, "starting application");

    Step::with(
        AppState::Starting {
            operation_id,
            started_at: ctx.now,
            waiters,
            failures,
        },
        vec![Effect::Start {
            operation_id,
            requested_at: ctx.now,
        }],
    )
}

pub(super) fn on_start_completed(
    state: AppState,
    operation_id: OperationId,
    result: Result<StartedProcess, AppError>,
    ctx: &Ctx<'_>,
) -> Step {
    match state {
        AppState::Starting {
            operation_id: current,
            waiters,
            mut failures,
            ..
        } if current == operation_id => match result {
            Ok(started) => enter_running(started, waiters, failures, ctx),
            Err(error) => {
                failures.record(ctx.now);
                warn!(
                    app = %ctx.spec.name,
                    %operation_id,
                    %error,
                    failures_in_window = failures.count_within(ctx.now, backoff::FAILURE_WINDOW),
                    "startup failed",
                );
                let effects = waiters
                    .into_iter()
                    .map(|waiter| Effect::RejectWaiter {
                        waiter,
                        error: AcquireError::StartFailed(error.clone()),
                    })
                    .collect();

                Step::with(
                    AppState::Failed {
                        error,
                        failed_at: ctx.now,
                        failures,
                    },
                    effects,
                )
            }
        },

        // stale operation. 그 사이 state가 바뀌었으므로 결과를 적용하지 않는다. (Invariant 8)
        other => discard_stale_start(other, operation_id, result, ctx),
    }
}

/// 이미 무효해진 startup 결과. 상태는 그대로 두고, 딸려 온 process만 거둔다. (Invariant 8)
fn discard_stale_start(
    state: AppState,
    operation_id: OperationId,
    result: Result<StartedProcess, AppError>,
    ctx: &Ctx<'_>,
) -> Step {
    match result {
        Ok(started) => {
            warn!(
                app = %ctx.spec.name,
                %operation_id,
                process = %started.handle.id(),
                "discarding stale startup result, stopping orphan process",
            );
            Step::with(
                state,
                vec![Effect::Stop {
                    process: started.handle,
                    operation_id: None,
                }],
            )
        }
        Err(_) => {
            debug!(app = %ctx.spec.name, %operation_id, "ignoring stale startup failure");
            Step::new(state)
        }
    }
}

fn enter_running(
    started: StartedProcess,
    waiters: Vec<Waiter>,
    failures: FailureHistory,
    ctx: &Ctx<'_>,
) -> Step {
    info!(
        app = %ctx.spec.name,
        process = %started.handle.id(),
        backend = %started.backend,
        waiters = waiters.len(),
        "application is running",
    );

    let mut effects = vec![Effect::Monitor {
        process: started.handle.clone(),
    }];

    let mut running = RunningState {
        process: started.handle,
        backend: started.backend,
        started_at: ctx.now,
        leases: HashSet::new(),
        last_request_at: ctx.now,
        idle_generation: ctx.ids.idle_generations.next_id(),
        failures,
    };

    // 모든 waiter가 같은 backend를 받고, 동시에 active request로 예약된다. (PRD §16)
    for waiter in waiters {
        let (next, effect) = grant(running, waiter, ctx);
        running = next;
        effects.push(effect);
    }

    if running.leases.is_empty() {
        let (next, effect) = arm_idle_timer(running, ctx);
        running = next;
        effects.push(effect);
    }

    Step::with(AppState::Running(running), effects)
}

fn begin_stop(
    process: ProcessHandle,
    waiters: Vec<Waiter>,
    reason: StopReason,
    ctx: &Ctx<'_>,
) -> Step {
    let operation_id = ctx.ids.operations.next_id();

    info!(
        app = %ctx.spec.name,
        %operation_id,
        process = %process.id(),
        %reason,
        "stopping application",
    );

    Step::with(
        AppState::Stopping {
            operation_id,
            process: process.clone(),
            waiters,
        },
        vec![Effect::Stop {
            process,
            operation_id: Some(operation_id),
        }],
    )
}

pub(super) fn on_stop_completed(
    state: AppState,
    operation_id: OperationId,
    result: Result<(), RuntimeError>,
    ctx: &Ctx<'_>,
) -> Step {
    if let Err(error) = &result {
        // stop 실패는 상태 전이를 막지 않는다. runtime이 강제 종료까지 책임진다. (PRD §30)
        warn!(app = %ctx.spec.name, %operation_id, %error, "stop reported an error");
    }

    match state {
        AppState::Stopping {
            operation_id: current,
            waiters,
            ..
        } if current == operation_id => finish_stop(waiters, ctx),
        other => Step::new(other),
    }
}

pub(super) fn on_process_exited(
    state: AppState,
    process_id: ProcessId,
    status: ExitStatus,
    ctx: &Ctx<'_>,
) -> Step {
    match state {
        AppState::Running(running) if running.process.id() == process_id => {
            // 기동은 성공하고 첫 요청에 죽는 application도 crash loop다. (PRD §77)
            let mut failures = running.failures;
            failures.record(ctx.now);

            warn!(
                app = %ctx.spec.name,
                process = %process_id,
                %status,
                leases = running.leases.len(),
                failures_in_window = failures.count_within(ctx.now, backoff::FAILURE_WINDOW),
                "application process exited unexpectedly",
            );
            // 살아 있던 lease는 모두 무효가 된다. (PRD §32)
            Step::new(AppState::Failed {
                error: AppError::ProcessExited(status.to_string()),
                failed_at: ctx.now,
                failures,
            })
        }

        // STOPPING 중의 exit은 정상 종료 경로다. (PRD §32)
        AppState::Stopping {
            process, waiters, ..
        } if process.id() == process_id => finish_stop(waiters, ctx),

        other => {
            debug!(
                app = %ctx.spec.name,
                process = %process_id,
                state = %other.name(),
                "ignoring stale process exit",
            );
            Step::new(other)
        }
    }
}

fn finish_stop(waiters: Vec<Waiter>, ctx: &Ctx<'_>) -> Step {
    // daemon이 내려가는 중이라면 재기동하지 않는다. 기다리던 요청은 실패시킨다.
    if ctx.shutting_down {
        let effects = waiters
            .into_iter()
            .map(|waiter| Effect::RejectWaiter {
                waiter,
                error: AcquireError::Unavailable,
            })
            .collect();
        return Step::with(AppState::Sleeping, effects);
    }

    if waiters.is_empty() {
        Step::new(AppState::Sleeping)
    } else {
        // stop을 기다리던 요청이 있으면 바로 다시 기동한다. waiter는 유실하지 않는다. (PRD §31)
        //
        // 이력을 초기화한다. 여기 도달했다는 것은 정상으로 돌다가 멈췄다는 뜻이다. (PRD §77)
        begin_start(waiters, FailureHistory::default(), ctx)
    }
}

fn arm_idle_timer(mut running: RunningState, ctx: &Ctx<'_>) -> (RunningState, Effect) {
    let generation = ctx.ids.idle_generations.next_id();
    running.idle_generation = generation;
    (running, Effect::ScheduleIdleTimer { generation })
}

/// idle timer 발화. 세 조건이 모두 맞아야 정지한다. (PRD §28–§29)
///
/// ```text
/// generation 일치      새 요청이 들어왔다면 이 timer는 이미 무효다
/// lease 없음           진행 중인 요청이 있으면 정지하지 않는다 (PRD §25)
/// 실제로 idle 경과      timer가 예정보다 일찍 깨어난 경우를 거른다
/// ```
pub(super) fn on_idle_timeout(state: AppState, generation: IdleGeneration, ctx: &Ctx<'_>) -> Step {
    let idle_timeout = ctx.spec.lifecycle.idle_timeout();

    // 세 조건 중 **실제로 막는 것은 뒤의 둘**이다. 뮤테이션으로 확인했다.
    //
    //   leases.is_empty() 제거   returning_one_of_many_leases_does_not_arm_the_timer 실패
    //   시간 검사 제거            an_early_idle_timer_does_not_stop_the_app 실패
    //   세대 비교 제거            아무 테스트도 실패하지 않는다
    //
    // 세대 비교가 중복인 이유가 있다. 옛 timer가 도착한 시점에 lease가 비어 있고
    // idle timeout도 지났다면, **어느 timer가 깨웠든 멈추는 것이 옳다.** 그래서
    // 행동으로 구분되는 경우가 없고 테스트를 쓸 수도 없다.
    //
    // 그래도 둔다. PRD §29·§68이 처방한 기제이고, 뒤의 두 조건이 약해지면 마지막
    // 방어선이 된다. 다만 **주된 보호로 착각하지 말 것.** §29를 읽고 "세대가
    // 막아 준다"고 믿으면 틀린 전제로 코드를 고치게 된다.
    match state {
        AppState::Running(running)
            if running.idle_generation == generation
                && running.leases.is_empty()
                && ctx.now.duration_since(running.last_request_at) >= idle_timeout =>
        {
            begin_stop(running.process, Vec::new(), StopReason::Idle, ctx)
        }

        other => {
            debug!(
                app = %ctx.spec.name,
                %generation,
                state = %other.name(),
                "ignoring stale idle timeout",
            );
            Step::new(other)
        }
    }
}

pub(super) fn on_stop(state: AppState, reason: StopReason, ctx: &Ctx<'_>) -> Step {
    match state {
        AppState::Running(running) => begin_stop(running.process, Vec::new(), reason, ctx),
        other => Step::new(other),
    }
}

/// 종료 중이라면 정리를 한 걸음 진행시킨다.
///
/// STARTING 중에 종료 요청이 오면 기동이 끝나기를 기다렸다가 곧바로 정지시킨다.
/// 여기서 끊어 버리면 방금 뜬 process를 아무도 거두지 않는다.
pub(super) fn settle_shutdown(state: AppState, ctx: &Ctx<'_>) -> Step {
    match state {
        AppState::Running(running) => {
            begin_stop(running.process, Vec::new(), StopReason::DaemonShutdown, ctx)
        }
        other => Step::new(other),
    }
}

/// actor loop가 끝난 뒤의 마지막 정리.
///
/// mailbox가 닫혀 loop가 끝나는 경로(`Shutdown` 없이)도 있으므로 best-effort로 남긴다.
pub(super) fn teardown(state: AppState) -> Vec<Effect> {
    match state {
        AppState::Starting { waiters, .. } | AppState::Stopping { waiters, .. } => waiters
            .into_iter()
            .map(|waiter| Effect::RejectWaiter {
                waiter,
                error: AcquireError::Unavailable,
            })
            .collect(),
        AppState::Running(running) => vec![Effect::Stop {
            process: running.process,
            operation_id: None,
        }],
        _ => Vec::new(),
    }
}

pub(super) fn status_of(state: &AppState, spec: &AppSpec) -> AppStatus {
    AppStatus {
        name: spec.name.clone(),
        state: state.name(),
        active_requests: state.active_requests(),
        pending_waiters: state.pending_waiters(),
        process_id: state.process_id(),
        os_pid: state.os_pid(),
        last_error: state.last_error(),
        since: state.uptime(),
    }
}

/// 상태를 전이 함수들 사이로 흘려보내는 얇은 껍데기.
///
/// I/O도 tokio도 모른다. 같은 이벤트 순서에는 같은 상태와 같은 effect 목록이 나온다.
/// 테스트는 [`Actor`](super::manager) 없이 이것만 조립해서 쓴다.
#[derive(Debug)]
pub(super) struct Machine {
    spec: std::sync::Arc<AppSpec>,
    state: AppState,
    ids: Ids,
    limits: Limits,
    /// 종료를 기다리는 쪽. process가 사라진 뒤에 [`Effect::ConfirmShutdown`]으로 나간다.
    shutdown_ack: Option<oneshot::Sender<()>>,
}

impl Machine {
    pub(super) fn new(spec: std::sync::Arc<AppSpec>, limits: Limits) -> Self {
        Self {
            spec,
            state: AppState::Sleeping,
            ids: Ids::new(),
            limits,
            shutdown_ack: None,
        }
    }

    /// 전이 테스트가 상태를 들여다보기 위한 창.
    #[cfg(test)]
    pub(super) const fn state(&self) -> &AppState {
        &self.state
    }

    /// 이벤트 하나를 처리한다. 수행할 effect와 loop 제어를 돌려준다.
    pub(super) fn step(
        &mut self,
        command: AppCommand,
        now: Instant,
        inputs: Inputs,
    ) -> (Vec<Effect>, ControlFlow<()>) {
        let ctx = Ctx {
            spec: &self.spec,
            ids: &self.ids,
            limits: self.limits,
            inputs,
            now,
            shutting_down: self.shutdown_ack.is_some(),
        };
        let state = std::mem::replace(&mut self.state, AppState::Sleeping);

        let mut step = match command {
            AppCommand::Acquire { reply } => on_acquire(state, reply, &ctx),
            AppCommand::RequestFinished { lease_id } => on_request_finished(state, lease_id, &ctx),
            AppCommand::StartCompleted {
                operation_id,
                result,
            } => on_start_completed(state, operation_id, result, &ctx),
            AppCommand::StopCompleted {
                operation_id,
                result,
            } => on_stop_completed(state, operation_id, result, &ctx),
            AppCommand::ProcessExited { process_id, status } => {
                on_process_exited(state, process_id, status, &ctx)
            }
            AppCommand::IdleTimeout { generation } => on_idle_timeout(state, generation, &ctx),
            AppCommand::Stop { reason } => on_stop(state, reason, &ctx),
            AppCommand::Status { reply } => {
                let status = status_of(&state, &self.spec);
                Step::with(state, vec![Effect::ReportStatus { reply, status }])
            }
            AppCommand::Shutdown { ack } => {
                // ack는 지금 보내지 않는다. process가 사라진 뒤에 보낸다. (Invariant 10)
                self.shutdown_ack = Some(ack);
                Step::new(state)
            }
        };

        self.state = std::mem::replace(&mut step.state, AppState::Sleeping);
        self.settle(step.effects, now)
    }

    /// 종료 중이면 정착할 때까지 밀고, 정착했으면 ack를 내보내며 loop를 끝낸다.
    fn settle(&mut self, mut effects: Vec<Effect>, now: Instant) -> (Vec<Effect>, ControlFlow<()>) {
        if self.shutdown_ack.is_none() {
            return (effects, ControlFlow::Continue(()));
        }

        let ctx = Ctx {
            spec: &self.spec,
            ids: &self.ids,
            limits: self.limits,
            inputs: Inputs::default(),
            now,
            shutting_down: true,
        };
        let state = std::mem::replace(&mut self.state, AppState::Sleeping);
        let mut step = settle_shutdown(state, &ctx);
        self.state = std::mem::replace(&mut step.state, AppState::Sleeping);
        effects.append(&mut step.effects);

        if !self.state.is_settled() {
            return (effects, ControlFlow::Continue(()));
        }

        if let Some(ack) = self.shutdown_ack.take() {
            effects.push(Effect::ConfirmShutdown { ack });
        }
        (effects, ControlFlow::Break(()))
    }

    /// loop가 끝난 뒤 남은 것을 정리한다.
    pub(super) fn teardown(&mut self) -> Vec<Effect> {
        teardown(std::mem::replace(&mut self.state, AppState::Sleeping))
    }
}

#[cfg(test)]
#[path = "transition_tests.rs"]
mod tests;
