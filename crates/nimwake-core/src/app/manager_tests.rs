//! AppManager white-box 테스트.
//!
//! 요구사항: PRD §39 Unit Tests (상태 전이 표 + stale event 표).
//!
//! actor에 command를 한 건씩 밀어 넣어 상태 전이를 결정적으로 검증한다.

use std::net::SocketAddr;

use super::*;
use crate::app::AppStateName;
use crate::id::ProcessId;
use crate::runtime::{detached_handle, Backend, FakeRuntime};

fn test_spec(idle_seconds: u64, max_pending_requests: usize) -> AppSpec {
    AppSpec::from_toml(&format!(
        r#"
name = "test"

[server]
host = "test.localhost"
port = 3101

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

struct Harness {
    actor: Actor,
    rx: mpsc::Receiver<AppCommand>,
    runtime: Arc<FakeRuntime>,
}

impl Harness {
    fn new() -> Self {
        Self::with_spec(test_spec(60, 256))
    }

    fn with_spec(spec: AppSpec) -> Self {
        let runtime = Arc::new(FakeRuntime::default());
        let (tx, rx) = mpsc::channel(256);

        let actor = Actor {
            spec: Arc::new(spec),
            runtime: Arc::clone(&runtime) as Arc<dyn Runtime>,
            self_tx: tx,
            state: AppState::Sleeping,
            operation_ids: OperationIdSeq::new(),
            lease_ids: LeaseIdSeq::new(),
            idle_generations: IdleGenerationSeq::new(),
        };

        Self { actor, rx, runtime }
    }

    fn state(&self) -> AppStateName {
        self.actor.state.name()
    }

    fn active_requests(&self) -> usize {
        self.actor.state.active_requests()
    }

    fn pending_waiters(&self) -> usize {
        self.actor.state.pending_waiters()
    }

    fn process_id(&self) -> Option<ProcessId> {
        self.actor.state.process_id()
    }

    fn idle_generation(&self) -> IdleGeneration {
        match &self.actor.state {
            AppState::Running(running) => running.idle_generation,
            other => panic!("not running, state is {}", other.name()),
        }
    }

    fn acquire(&mut self) -> oneshot::Receiver<Result<RequestLease, AcquireError>> {
        let (reply, response) = oneshot::channel();
        let _ = self.actor.handle(AppCommand::Acquire { reply });
        response
    }

    fn send(&mut self, command: AppCommand) {
        let _ = self.actor.handle(command);
    }

    /// mailbox에서 event 한 건을 꺼내 actor에 전달한다.
    async fn pump(&mut self) {
        let command = self.rx.recv().await.expect("actor mailbox is open");
        let _ = self.actor.handle(command);
    }

    /// 목표 상태에 도달할 때까지 event를 밀어 넣는다.
    ///
    /// stop 완료 시 `StopCompleted`와 `ProcessExited`가 둘 다 도착하는 것처럼
    /// 같은 전이에 대해 event가 여러 건 올 수 있으므로 개수를 세지 않는다.
    async fn pump_until(&mut self, target: AppStateName) {
        for _ in 0..16 {
            if self.state() == target {
                return;
            }
            self.pump().await;
        }
        panic!("state never reached {target}, still {}", self.state());
    }

    /// SLEEPING에서 RUNNING까지 진행시키고 lease 하나를 돌려준다.
    async fn start(&mut self) -> RequestLease {
        let response = self.acquire();
        self.pump().await;
        response
            .await
            .expect("reply delivered")
            .expect("startup succeeded")
    }

    fn fake_started(process_id: u64) -> StartedProcess {
        StartedProcess {
            handle: detached_handle(ProcessId::from_raw(process_id)),
            backend: Backend::new(SocketAddr::from(([127, 0, 0, 1], 3101))),
        }
    }
}

// --------------------------------------------------------- 상태 전이 (PRD §39)

#[tokio::test]
async fn sleeping_plus_acquire_enters_starting() {
    let mut h = Harness::new();
    let _response = h.acquire();
    assert_eq!(h.state(), AppStateName::Starting);
    assert_eq!(h.pending_waiters(), 1);
}

#[tokio::test]
async fn starting_plus_success_enters_running() {
    let mut h = Harness::new();
    let lease = h.start().await;
    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.active_requests(), 1);
    assert_eq!(lease.backend().addr().port(), 3101);
}

#[tokio::test]
async fn starting_plus_failure_enters_failed() {
    let mut h = Harness::new();
    h.runtime.fail_next_starts(1);

    let response = h.acquire();
    h.pump().await;

    assert_eq!(h.state(), AppStateName::Failed);
    assert!(matches!(
        response.await.expect("reply delivered"),
        Err(AcquireError::StartFailed(_))
    ));
}

#[tokio::test]
async fn running_plus_acquire_reuses_process() {
    let mut h = Harness::new();
    let _first = h.start().await;
    let process = h.process_id();

    let response = h.acquire();
    let _second = response
        .await
        .expect("reply delivered")
        .expect("lease issued");

    assert_eq!(h.runtime.start_count(), 1);
    assert_eq!(h.active_requests(), 2);
    assert_eq!(h.process_id(), process);
}

#[tokio::test]
async fn running_plus_crash_enters_failed() {
    let mut h = Harness::new();
    let _lease = h.start().await;
    let process = h.process_id().expect("running has a process");

    h.runtime.crash(process);
    h.pump().await; // monitor task가 보고한 ProcessExited

    assert_eq!(h.state(), AppStateName::Failed);
    assert_eq!(h.active_requests(), 0);
}

#[tokio::test]
async fn failed_plus_acquire_enters_starting() {
    let mut h = Harness::new();
    h.runtime.fail_next_starts(1);
    let response = h.acquire();
    h.pump().await;
    let _ = response.await;
    assert_eq!(h.state(), AppStateName::Failed);

    let _retry = h.acquire();
    assert_eq!(h.state(), AppStateName::Starting);
}

#[tokio::test(start_paused = true)]
async fn idle_with_no_active_lease_enters_stopping() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let lease = h.start().await;

    drop(lease);
    h.pump().await; // RequestFinished

    tokio::time::advance(Duration::from_secs(2)).await;
    h.pump().await; // IdleTimeout

    assert_eq!(h.state(), AppStateName::Stopping);
}

#[tokio::test(start_paused = true)]
async fn idle_with_active_lease_stays_running() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let _lease = h.start().await;

    let generation = h.idle_generation();
    tokio::time::advance(Duration::from_secs(2)).await;
    h.send(AppCommand::IdleTimeout { generation });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.active_requests(), 1);
}

#[tokio::test(start_paused = true)]
async fn stale_idle_timer_is_ignored() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    let lease = h.start().await;
    drop(lease);
    h.pump().await; // RequestFinished -> idle timer 예약

    let stale = h.idle_generation();

    // 새 요청이 들어오면 세대가 올라가고 기존 timer는 무효가 된다. (PRD §29)
    let _fresh = h.acquire();
    assert_ne!(h.idle_generation(), stale);

    h.send(AppCommand::IdleTimeout { generation: stale });
    assert_eq!(h.state(), AppStateName::Running);
}

#[tokio::test(start_paused = true)]
async fn stopping_plus_acquire_restarts_after_stop() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    h.runtime.set_stop_delay(Duration::from_millis(50));

    let lease = h.start().await;
    let first_process = h.process_id();
    drop(lease);
    h.pump().await; // RequestFinished

    tokio::time::advance(Duration::from_secs(2)).await;
    h.pump().await; // IdleTimeout -> STOPPING
    assert_eq!(h.state(), AppStateName::Stopping);

    // STOPPING 중 도착한 요청은 실패시키지 않고 waiter로 등록한다. (PRD §31)
    let response = h.acquire();
    assert_eq!(h.state(), AppStateName::Stopping);
    assert_eq!(h.pending_waiters(), 1);

    h.pump_until(AppStateName::Starting).await;
    assert_eq!(
        h.pending_waiters(),
        1,
        "waiter는 재기동으로 이월되어야 한다"
    );

    h.pump_until(AppStateName::Running).await;
    let _lease = response
        .await
        .expect("waiter survived the stop")
        .expect("restart succeeded");
    assert_eq!(h.runtime.start_count(), 2);
    assert_ne!(h.process_id(), first_process);
}

// ---------------------------------------------------- stale event (Invariant 8)

#[tokio::test]
async fn stale_start_completed_is_ignored() {
    let mut h = Harness::new();
    let _lease = h.start().await;
    let process = h.process_id();

    h.send(AppCommand::StartCompleted {
        operation_id: OperationId::from_raw(9999),
        result: Ok(Harness::fake_started(4242)),
    });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.process_id(), process);
}

#[tokio::test]
async fn stale_start_completed_stops_the_orphan_process() {
    let mut h = Harness::new();
    let _lease = h.start().await;

    let orphan = h
        .runtime
        .start(&h.actor.spec)
        .await
        .expect("fake start succeeds");
    assert_eq!(h.runtime.alive_processes(), 2);

    h.send(AppCommand::StartCompleted {
        operation_id: OperationId::from_raw(9999),
        result: Ok(orphan),
    });

    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.runtime.alive_processes(), 1);
}

#[tokio::test(start_paused = true)]
async fn stale_stop_completed_is_ignored() {
    let mut h = Harness::with_spec(test_spec(1, 256));
    h.runtime.set_stop_delay(Duration::from_millis(50));

    let lease = h.start().await;
    drop(lease);
    h.pump().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    h.pump().await; // -> STOPPING

    h.send(AppCommand::StopCompleted {
        operation_id: OperationId::from_raw(9999),
        result: Ok(()),
    });

    assert_eq!(h.state(), AppStateName::Stopping);
}

#[tokio::test]
async fn stale_process_exit_is_ignored() {
    let mut h = Harness::new();
    let _lease = h.start().await;
    let process = h.process_id();

    h.send(AppCommand::ProcessExited {
        process_id: ProcessId::from_raw(9999),
        status: ExitStatus::from_code(1),
    });

    assert_eq!(h.state(), AppStateName::Running);
    assert_eq!(h.process_id(), process);
    assert_eq!(h.active_requests(), 1);
}

#[tokio::test]
async fn duplicate_request_finished_does_not_change_the_counter() {
    let mut h = Harness::new();
    let lease = h.start().await;
    let lease_id = lease.id();
    assert_eq!(h.active_requests(), 1);

    h.send(AppCommand::RequestFinished { lease_id });
    assert_eq!(h.active_requests(), 0);

    h.send(AppCommand::RequestFinished { lease_id });
    assert_eq!(h.active_requests(), 0);
    assert_eq!(h.state(), AppStateName::Running);

    drop(lease);
}

#[tokio::test]
async fn startup_failure_rejects_every_waiter() {
    let mut h = Harness::new();
    h.runtime.fail_next_starts(1);
    h.runtime.set_start_delay(Duration::from_millis(1));

    let responses: Vec<_> = (0..16).map(|_| h.acquire()).collect();
    assert_eq!(h.pending_waiters(), 16);

    h.pump().await;
    assert_eq!(h.state(), AppStateName::Failed);

    for response in responses {
        assert!(matches!(
            response.await.expect("reply delivered"),
            Err(AcquireError::StartFailed(_))
        ));
    }
}

#[tokio::test]
async fn pending_waiter_limit_rejects_with_overloaded() {
    let mut h = Harness::with_spec(test_spec(60, 2));
    h.runtime.set_start_delay(Duration::from_secs(30));

    let _first = h.acquire();
    let _second = h.acquire();
    let third = h.acquire();

    assert_eq!(h.pending_waiters(), 2);
    assert!(matches!(
        third.await.expect("reply delivered"),
        Err(AcquireError::Overloaded)
    ));
}
