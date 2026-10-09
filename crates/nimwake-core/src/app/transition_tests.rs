//! 상태 전이 테스트.
//!
//! 요구사항: PRD §39 Unit Tests (상태 전이 표 + stale event 표).
//!
//! 전이는 순수 함수이므로 여기에는 async runtime도, fake runtime도, 이벤트 루프도 없다.
//! 상태와 이벤트를 넣고 **새 상태와 예약된 effect**를 바로 단언한다.
//!
//! 시각을 주입받는 덕분에 idle 판정도 시계를 조작하지 않고 검증한다.

use std::net::SocketAddr;
use std::time::Duration;

use super::*;
use crate::app::AppStateName;
use crate::id::ProcessId;
use crate::runtime::{detached_handle, Backend};
use crate::RequestLease;

const BACKEND_PORT: u16 = 3101;

fn test_spec(idle_seconds: u64, max_pending_requests: usize) -> AppSpec {
    AppSpec::from_toml(&format!(
        r#"
name = "test"

[server]
host = "test.localhost"
port = {BACKEND_PORT}

[runtime]
command = ["true"]
working_dir = "."

[lifecycle]
idle_timeout_seconds = {idle_seconds}
startup_timeout_seconds = 5
shutdown_timeout_seconds = 5
max_pending_requests = {max_pending_requests}
"#
    ))
    .expect("test spec parses")
}

fn started(process_id: u64) -> StartedProcess {
    StartedProcess {
        handle: detached_handle(ProcessId::from_raw(process_id)),
        backend: Backend::new(SocketAddr::from(([127, 0, 0, 1], BACKEND_PORT))),
        timings: crate::trace::StartTimings::instant(),
    }
}

/// [`Machine`]에 이벤트를 넣고 결과를 들여다보는 도구.
///
/// actor도 채널도 없다. 시각은 테스트가 직접 굴린다.
struct Harness {
    machine: Machine,
    now: Instant,
}

impl Harness {
    fn new() -> Self {
        Self::with_spec(test_spec(60, 256))
    }

    fn with_spec(spec: AppSpec) -> Self {
        Self {
            machine: Machine::new(std::sync::Arc::new(spec)),
            now: Instant::now(),
        }
    }

    fn step(&mut self, command: AppCommand) -> Vec<Effect> {
        self.machine.step(command, self.now).0
    }

    /// 종료 여부까지 함께 본다.
    fn step_full(&mut self, command: AppCommand) -> (Vec<Effect>, ControlFlow<()>) {
        self.machine.step(command, self.now)
    }

    /// 시계를 앞으로 돌린다. 실제로 기다리지 않는다.
    fn advance(&mut self, by: Duration) {
        self.now += by;
    }

    fn acquire(
        &mut self,
    ) -> (
        Vec<Effect>,
        oneshot::Receiver<Result<RequestLease, AcquireError>>,
    ) {
        let (reply, response) = oneshot::channel();
        (self.step(AppCommand::Acquire { reply }), response)
    }

    /// SLEEPING에서 RUNNING까지 진행시킨다. 발급된 lease id를 돌려준다.
    fn start(&mut self) -> LeaseId {
        let (effects, _response) = self.acquire();
        let operation_id = only_start(&effects);

        let effects = self.step(AppCommand::StartCompleted {
            operation_id,
            result: Ok(started(1)),
        });
        granted(&effects)
            .first()
            .copied()
            .expect("startup grants the waiting request a lease")
    }

    fn state(&self) -> AppStateName {
        self.machine.state().name()
    }

    fn active_requests(&self) -> usize {
        self.machine.state().active_requests()
    }

    fn pending_waiters(&self) -> usize {
        self.machine.state().pending_waiters()
    }

    fn process_id(&self) -> Option<ProcessId> {
        self.machine.state().process_id()
    }

    fn idle_generation(&self) -> IdleGeneration {
        match self.machine.state() {
            AppState::Running(running) => running.idle_generation,
            other => panic!("not running, state is {}", other.name()),
        }
    }
}

// 아래 도구들은 effect 목록에서 필요한 것만 꺼낸다.
// 테스트 본문이 Effect 패턴 매칭으로 덮이지 않게 하려는 것이다.

fn only_start(effects: &[Effect]) -> OperationId {
    let ids: Vec<_> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Start { operation_id, .. } => Some(*operation_id),
            _ => None,
        })
        .collect();

    assert_eq!(ids.len(), 1, "expected exactly one Start in {effects:?}");
    ids[0]
}

fn stops(effects: &[Effect]) -> Vec<Option<OperationId>> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Stop { operation_id, .. } => Some(*operation_id),
            _ => None,
        })
        .collect()
}

fn granted(effects: &[Effect]) -> Vec<LeaseId> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::GrantLease { lease_id, .. } => Some(*lease_id),
            _ => None,
        })
        .collect()
}

fn rejections(effects: &[Effect]) -> Vec<&AcquireError> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::RejectWaiter { error, .. } => Some(error),
            _ => None,
        })
        .collect()
}

fn armed_idle_timers(effects: &[Effect]) -> Vec<IdleGeneration> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ScheduleIdleTimer { generation } => Some(*generation),
            _ => None,
        })
        .collect()
}

fn monitored(effects: &[Effect]) -> usize {
    effects
        .iter()
        .filter(|effect| matches!(effect, Effect::Monitor { .. }))
        .count()
}

// PRD §39 상태 전이 표. 표의 각 행이 아래 테스트 하나에 대응한다.

#[test]
fn sleeping_plus_acquire_enters_starting() {
    let mut h = Harness::new();
    let (effects, _response) = h.acquire();

    assert_eq!(h.state(), AppStateName::Starting);
    assert_eq!(h.pending_waiters(), 1);
    only_start(&effects);
}

#[test]
fn starting_plus_success_enters_running() {
    let mut h = Harness::new();
    let (effects, _response) = h.acquire();
    let operation_id = only_start(&effects);

    let effects = h.step(AppCommand::StartCompleted {
        operation_id,
        result: Ok(started(1)),
    });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.active_requests(), 1);
    assert_eq!(monitored(&effects), 1, "RUNNING 진입 시 감시를 건다");
    assert_eq!(granted(&effects).len(), 1);
    assert!(
        armed_idle_timers(&effects).is_empty(),
        "lease가 있으면 idle timer를 걸지 않는다",
    );
}

#[test]
fn starting_plus_failure_enters_failed() {
    let mut h = Harness::new();
    let (effects, _response) = h.acquire();
    let operation_id = only_start(&effects);

    let effects = h.step(AppCommand::StartCompleted {
        operation_id,
        result: Err(AppError::StartupTimeout(Duration::from_secs(5))),
    });

    assert_eq!(h.state(), AppStateName::Failed);
    assert!(matches!(
        rejections(&effects).as_slice(),
        [AcquireError::StartFailed(_)]
    ));
}

#[test]
fn running_plus_acquire_reuses_the_process() {
    let mut h = Harness::new();
    let first = h.start();
    let process = h.process_id();

    let (effects, _response) = h.acquire();

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.active_requests(), 2);
    assert_eq!(h.process_id(), process);

    let second = granted(&effects);
    assert_eq!(second.len(), 1);
    assert_ne!(second[0], first, "lease는 매번 새로 발급된다");
    assert!(
        effects.iter().all(|e| !matches!(e, Effect::Start { .. })),
        "이미 RUNNING이면 다시 기동하지 않는다",
    );
}

#[test]
fn running_plus_process_exit_enters_failed() {
    let mut h = Harness::new();
    h.start();
    let process = h.process_id().expect("running has a process");

    h.step(AppCommand::ProcessExited {
        process_id: process,
        status: ExitStatus::from_code(1),
    });

    assert_eq!(h.state(), AppStateName::Failed);
    // 살아 있던 lease는 모두 무효가 된다. (PRD §32)
    assert_eq!(h.active_requests(), 0);
}

#[test]
fn failed_plus_acquire_enters_starting() {
    let mut h = Harness::new();
    let (effects, _response) = h.acquire();
    let operation_id = only_start(&effects);
    h.step(AppCommand::StartCompleted {
        operation_id,
        result: Err(AppError::StartFailed("boom".to_owned())),
    });
    assert_eq!(h.state(), AppStateName::Failed);

    let (effects, _retry) = h.acquire();

    assert_eq!(h.state(), AppStateName::Starting);
    only_start(&effects);
}

// idle timer. 정지 조건은 세 가지가 모두 맞을 때뿐이다. (PRD §28–§29)

#[test]
fn returning_the_last_lease_arms_the_idle_timer() {
    let mut h = Harness::new();
    let lease_id = h.start();

    let effects = h.step(AppCommand::RequestFinished { lease_id });

    assert_eq!(h.active_requests(), 0);
    assert_eq!(
        armed_idle_timers(&effects),
        vec![h.idle_generation()],
        "마지막 lease를 반납하면 idle timer를 건다",
    );
}

#[test]
fn returning_one_of_many_leases_does_not_arm_the_timer() {
    let mut h = Harness::new();
    let first = h.start();
    let (_effects, _response) = h.acquire();
    assert_eq!(h.active_requests(), 2);

    let effects = h.step(AppCommand::RequestFinished { lease_id: first });

    assert_eq!(h.active_requests(), 1);
    assert!(
        armed_idle_timers(&effects).is_empty(),
        "아직 진행 중인 요청이 있으면 idle timer를 걸지 않는다",
    );
}

#[test]
fn idle_with_no_active_lease_enters_stopping() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let lease_id = h.start();

    let effects = h.step(AppCommand::RequestFinished { lease_id });
    let generation = armed_idle_timers(&effects)[0];

    h.advance(Duration::from_secs(2));
    let effects = h.step(AppCommand::IdleTimeout { generation });

    assert_eq!(h.state(), AppStateName::Stopping);
    assert!(matches!(stops(&effects).as_slice(), [Some(_)]));
}

#[test]
fn idle_with_an_active_lease_stays_running() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    h.start();

    let generation = h.idle_generation();
    h.advance(Duration::from_secs(2));
    h.step(AppCommand::IdleTimeout { generation });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.active_requests(), 1);
}

/// timer가 예정보다 일찍 깨어나도 정지하지 않는다.
///
/// 시각을 주입받기 때문에 쓸 수 있는 테스트다.
#[test]
fn an_early_idle_timer_does_not_stop_the_app() {
    let mut h = Harness::with_spec(test_spec(60, 256));
    let lease_id = h.start();
    let effects = h.step(AppCommand::RequestFinished { lease_id });
    let generation = armed_idle_timers(&effects)[0];

    h.advance(Duration::from_secs(59));
    h.step(AppCommand::IdleTimeout { generation });

    assert_eq!(h.state(), AppStateName::Running);
}

#[test]
fn a_stale_idle_timer_is_ignored() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let lease_id = h.start();
    let effects = h.step(AppCommand::RequestFinished { lease_id });
    let stale = armed_idle_timers(&effects)[0];

    // 새 요청이 들어오면 세대가 올라가고 기존 timer는 무효가 된다. (PRD §29)
    let (_effects, _fresh) = h.acquire();
    assert_ne!(h.idle_generation(), stale);

    h.advance(Duration::from_secs(2));
    h.step(AppCommand::IdleTimeout { generation: stale });

    assert_eq!(h.state(), AppStateName::Running);
}

#[test]
fn stopping_plus_acquire_restarts_after_the_stop_completes() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let lease_id = h.start();
    let first_process = h.process_id();

    let effects = h.step(AppCommand::RequestFinished { lease_id });
    let generation = armed_idle_timers(&effects)[0];
    h.advance(Duration::from_secs(2));
    let effects = h.step(AppCommand::IdleTimeout { generation });
    assert_eq!(h.state(), AppStateName::Stopping);
    let stop_operation = stops(&effects)[0].expect("stop reports back");

    // STOPPING 중 도착한 요청은 실패시키지 않고 waiter로 등록한다. (PRD §31)
    let (effects, _response) = h.acquire();
    assert_eq!(h.state(), AppStateName::Stopping);
    assert_eq!(h.pending_waiters(), 1);
    assert!(rejections(&effects).is_empty());

    let effects = h.step(AppCommand::StopCompleted {
        operation_id: stop_operation,
        result: Ok(()),
    });

    assert_eq!(h.state(), AppStateName::Starting);
    assert_eq!(h.pending_waiters(), 1, "waiter는 재기동으로 이월된다");

    let operation_id = only_start(&effects);
    h.step(AppCommand::StartCompleted {
        operation_id,
        result: Ok(started(2)),
    });

    assert_eq!(h.state(), AppStateName::Running);
    assert_ne!(h.process_id(), first_process);
}

// PRD §39 stale event 표.
// actor 밖에서 온 event는 도착 시점에 이미 무효일 수 있다. 그때는 상태를 바꾸지 않는다. (Invariant 8)

#[test]
fn a_stale_start_completed_is_ignored() {
    let mut h = Harness::new();
    h.start();
    let process = h.process_id();

    h.step(AppCommand::StartCompleted {
        operation_id: OperationId::from_raw(9999),
        result: Err(AppError::StartFailed("late failure".to_owned())),
    });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.process_id(), process);
}

/// stale한 startup 결과에 딸려 온 process는 아무도 소유하지 않는다. 반드시 거둬야 한다.
#[test]
fn a_stale_start_completed_stops_the_orphan_process() {
    let mut h = Harness::new();
    h.start();
    let process = h.process_id();

    let effects = h.step(AppCommand::StartCompleted {
        operation_id: OperationId::from_raw(9999),
        result: Ok(started(4242)),
    });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.process_id(), process, "현재 process는 그대로다");
    assert_eq!(
        stops(&effects),
        vec![None],
        "고아 process는 보고 없이 정리만 한다",
    );
}

#[test]
fn a_stale_stop_completed_is_ignored() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let lease_id = h.start();
    let effects = h.step(AppCommand::RequestFinished { lease_id });
    let generation = armed_idle_timers(&effects)[0];
    h.advance(Duration::from_secs(2));
    h.step(AppCommand::IdleTimeout { generation });
    assert_eq!(h.state(), AppStateName::Stopping);

    h.step(AppCommand::StopCompleted {
        operation_id: OperationId::from_raw(9999),
        result: Ok(()),
    });

    assert_eq!(h.state(), AppStateName::Stopping);
}

#[test]
fn a_stale_process_exit_is_ignored() {
    let mut h = Harness::new();
    h.start();
    let process = h.process_id();

    h.step(AppCommand::ProcessExited {
        process_id: ProcessId::from_raw(9999),
        status: ExitStatus::from_code(1),
    });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.process_id(), process);
    assert_eq!(h.active_requests(), 1);
}

/// 정수 카운터였다면 중복 반납이 그대로 감소한다. 집합이어야 무시할 수 있다. (D16)
#[test]
fn a_duplicate_request_finished_does_not_change_the_counter() {
    let mut h = Harness::new();
    let lease_id = h.start();
    assert_eq!(h.active_requests(), 1);

    h.step(AppCommand::RequestFinished { lease_id });
    assert_eq!(h.active_requests(), 0);

    let effects = h.step(AppCommand::RequestFinished { lease_id });
    assert_eq!(h.active_requests(), 0);
    assert_eq!(h.state(), AppStateName::Running);
    assert!(
        armed_idle_timers(&effects).is_empty(),
        "중복 반납은 idle timer를 다시 걸지 않는다",
    );
}

// 대기열 상한. 모든 대기열은 bounded다. (Invariant 9 / PRD §69)

#[test]
fn startup_failure_rejects_every_waiter() {
    let mut h = Harness::new();
    let (effects, _first) = h.acquire();
    let operation_id = only_start(&effects);

    let _rest: Vec<_> = (0..15).map(|_| h.acquire()).collect();
    assert_eq!(h.pending_waiters(), 16);

    let effects = h.step(AppCommand::StartCompleted {
        operation_id,
        result: Err(AppError::StartFailed("boom".to_owned())),
    });

    assert_eq!(h.state(), AppStateName::Failed);
    assert_eq!(rejections(&effects).len(), 16, "한 명도 남기지 않는다");
}

#[test]
fn the_pending_waiter_limit_rejects_with_overloaded() {
    let mut h = Harness::with_spec(test_spec(60, 2));

    let (_effects, _first) = h.acquire();
    let (_effects, _second) = h.acquire();
    let (effects, _third) = h.acquire();

    assert_eq!(h.pending_waiters(), 2);
    assert!(matches!(
        rejections(&effects).as_slice(),
        [AcquireError::Overloaded]
    ));
}

// daemon 종료. child의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10)

#[test]
fn shutdown_while_sleeping_confirms_immediately() {
    let mut h = Harness::new();
    let (ack, _acked) = oneshot::channel();

    let (effects, control) = h.step_full(AppCommand::Shutdown { ack });

    assert_eq!(control, ControlFlow::Break(()));
    assert!(effects
        .iter()
        .any(|e| matches!(e, Effect::ConfirmShutdown { .. })));
}

/// RUNNING이면 먼저 정지시키고, **process가 사라진 뒤에야** ack를 보낸다.
///
/// 여기서 곧바로 ack하면 daemon이 먼저 종료해 shutdown timeout이 통째로 무시된다.
#[test]
fn shutdown_while_running_stops_first_then_confirms() {
    let mut h = Harness::new();
    h.start();
    let (ack, _acked) = oneshot::channel();

    let (effects, control) = h.step_full(AppCommand::Shutdown { ack });

    assert_eq!(control, ControlFlow::Continue(()), "아직 끝나면 안 된다");
    assert_eq!(h.state(), AppStateName::Stopping);
    let stop_operation = stops(&effects)[0].expect("stop reports back");
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::ConfirmShutdown { .. })),
        "process가 살아 있는 동안에는 확인해 주지 않는다",
    );

    let (effects, control) = h.step_full(AppCommand::StopCompleted {
        operation_id: stop_operation,
        result: Ok(()),
    });

    assert_eq!(control, ControlFlow::Break(()));
    assert_eq!(h.state(), AppStateName::Sleeping);
    assert!(effects
        .iter()
        .any(|e| matches!(e, Effect::ConfirmShutdown { .. })));
}

/// 종료 중에는 STOPPING waiter를 재기동으로 넘기지 않는다.
#[test]
fn shutdown_rejects_waiters_instead_of_restarting() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    h.start();

    let (ack, _acked) = oneshot::channel();
    let (effects, _control) = h.step_full(AppCommand::Shutdown { ack });
    let stop_operation = stops(&effects)[0].expect("stop reports back");

    let (_effects, _waiting) = h.acquire();
    assert_eq!(h.pending_waiters(), 1);

    let (effects, control) = h.step_full(AppCommand::StopCompleted {
        operation_id: stop_operation,
        result: Ok(()),
    });

    assert_eq!(control, ControlFlow::Break(()));
    assert_eq!(h.state(), AppStateName::Sleeping);
    assert!(
        effects.iter().all(|e| !matches!(e, Effect::Start { .. })),
        "종료 중에는 다시 기동하지 않는다",
    );
    assert!(matches!(
        rejections(&effects).as_slice(),
        [AcquireError::Unavailable]
    ));
}

/// mailbox가 닫혀 loop가 끝나는 경로. 남은 waiter를 버려두지 않는다.
#[test]
fn teardown_rejects_pending_waiters() {
    let mut h = Harness::new();
    let (_effects, _waiting) = h.acquire();
    assert_eq!(h.pending_waiters(), 1);

    let effects = h.machine.teardown();

    assert!(matches!(
        rejections(&effects).as_slice(),
        [AcquireError::Unavailable]
    ));
}

#[test]
fn teardown_stops_a_running_process() {
    let mut h = Harness::new();
    h.start();

    let effects = h.machine.teardown();

    assert_eq!(stops(&effects), vec![None]);
}

// 상태 조회. 테스트가 내부 필드를 들여다보지 않게 하려고 넣은 경로다. (D19)

#[test]
fn status_reports_the_current_state() {
    let mut h = Harness::new();
    h.start();

    let (reply, _response) = oneshot::channel();
    let effects = h.step(AppCommand::Status { reply });

    let status = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::ReportStatus { status, .. } => Some(status),
            _ => None,
        })
        .expect("status is reported");

    assert_eq!(status.name, "test");
    assert_eq!(status.state, AppStateName::Running);
    assert_eq!(status.active_requests, 1);
    assert!(status.process_id.is_some());
}

/// SLEEPING을 거쳐 다시 올라온 application에 **이전 incarnation의** idle timer가
/// 늦게 도착할 수 있다. 그것도 무효여야 한다. (PRD §29)
///
/// `a_stale_idle_timer_is_ignored`가 보는 것은 RUNNING 안에서 세대가 올라간
/// 경우다. 이쪽은 RUNNING이 한 번 끝나고 새로 시작된 경우다.
///
/// PRD는 "`IdleGeneration`은 `Running` state가 소유하므로 자동으로 무효가 된다"고
/// 적는다. 실제로 무효인 이유는 조금 다르다. 세대 수열이 `Ctx`에 있어 **application
/// 수명 전체에 걸쳐 단조 증가**하기 때문이다. 수열을 `Running` 안으로 옮기면
/// 새 incarnation이 같은 번호를 다시 내주고 이 테스트가 깨진다.
#[test]
fn an_idle_timer_from_a_previous_incarnation_is_ignored() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let lease_id = h.start();

    // 첫 incarnation에서 timer를 하나 무장시킨다.
    let effects = h.step(AppCommand::RequestFinished { lease_id });
    let previous = armed_idle_timers(&effects)[0];

    // 그 timer로 실제로 잠든다.
    h.advance(Duration::from_secs(2));
    let effects = h.step(AppCommand::IdleTimeout {
        generation: previous,
    });
    assert_eq!(h.state(), AppStateName::Stopping);
    let stop_operation = stops(&effects)[0].expect("stop reports back");

    h.step(AppCommand::StopCompleted {
        operation_id: stop_operation,
        result: Ok(()),
    });
    assert_eq!(h.state(), AppStateName::Sleeping);

    // 두 번째 incarnation.
    let (effects, _response) = h.acquire();
    let operation_id = only_start(&effects);
    h.step(AppCommand::StartCompleted {
        operation_id,
        result: Ok(started(2)),
    });
    assert_eq!(h.state(), AppStateName::Running);

    // 세대가 다시 쓰이지 않았다. 여기가 핵심이다.
    assert_ne!(
        h.idle_generation(),
        previous,
        "a new incarnation must not reuse a previous generation",
    );

    // 이전 incarnation의 timer가 아주 늦게 도착한다.
    h.advance(Duration::from_secs(2));
    h.step(AppCommand::IdleTimeout {
        generation: previous,
    });

    assert_eq!(
        h.state(),
        AppStateName::Running,
        "a timer from a previous incarnation must not stop the app",
    );
    assert_eq!(h.active_requests(), 1, "the live lease must survive");
}
