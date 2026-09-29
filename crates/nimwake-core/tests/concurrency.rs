//! Concurrency tests.
//!
//! 요구사항: PRD §40 Concurrency Tests.
//!
//! Nimwake에서 concurrency test는 first-class correctness test다.
//! `v0.0.1`의 핵심 acceptance는 다음 한 줄이다.
//!
//! ```text
//! 1000 concurrent acquire  ->  FakeRuntime.start_count == 1
//! ```

mod common;

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

use nimwake_core::app;
use nimwake_core::runtime::FakeRuntime;
use nimwake_core::{AcquireError, AppStateName, RequestLease, Runtime};

const CALLERS: usize = 1_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thousand_concurrent_acquires_start_exactly_one_process() {
    let runtime = Arc::new(FakeRuntime::default());
    // 모든 호출자가 STARTING 구간에 겹치도록 startup을 충분히 늦춘다.
    runtime.set_start_delay(Duration::from_millis(50));

    let handle = app::spawn_with_capacity(
        Arc::new(common::spec(60, CALLERS * 2)),
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        CALLERS * 2,
    );

    let mut callers = JoinSet::new();
    for _ in 0..CALLERS {
        let handle = handle.clone();
        callers.spawn(async move { handle.acquire().await });
    }

    let mut leases: Vec<RequestLease> = Vec::with_capacity(CALLERS);
    while let Some(joined) = callers.join_next().await {
        let lease = joined
            .expect("caller task did not panic")
            .expect("acquire succeeded");
        leases.push(lease);
    }

    // cold-start deduplication: 요청이 몇 개든 startup은 한 번이다. (Invariant 4)
    assert_eq!(runtime.start_count(), 1, "startup must happen exactly once");
    assert_eq!(
        runtime.alive_processes(),
        1,
        "at most one process (Invariant 1)"
    );

    // 모든 waiter가 같은 backend를 받는다. (PRD §16)
    let backend = leases[0].backend();
    assert!(leases.iter().all(|lease| lease.backend() == backend));

    // lease id는 모두 고유해야 한다.
    let unique: std::collections::HashSet<_> = leases.iter().map(RequestLease::id).collect();
    assert_eq!(unique.len(), CALLERS, "every caller gets a distinct lease");

    // 발급된 lease 수와 active_requests가 일치한다. (Invariant 7)
    let status = handle.status().await.expect("manager is alive");
    assert_eq!(status.state, AppStateName::Running);
    assert_eq!(status.active_requests, CALLERS);

    // 모든 lease를 반납하면 counter가 정확히 0으로 돌아온다.
    drop(leases);
    common::wait_for_active_requests(&handle, 0).await;

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_acquires_on_a_running_app_reuse_the_process() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = app::spawn_with_capacity(
        Arc::new(common::spec(60, CALLERS * 2)),
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        CALLERS * 2,
    );

    // 먼저 하나를 깨워 RUNNING으로 만든다.
    let warm = handle.acquire().await.expect("first acquire succeeds");

    let mut callers = JoinSet::new();
    for _ in 0..CALLERS {
        let handle = handle.clone();
        callers.spawn(async move { handle.acquire().await });
    }

    let mut leases = Vec::with_capacity(CALLERS);
    while let Some(joined) = callers.join_next().await {
        leases.push(
            joined
                .expect("caller task did not panic")
                .expect("acquire succeeded"),
        );
    }

    assert_eq!(runtime.start_count(), 1, "warm path must not start again");

    drop(leases);
    drop(warm);
    common::wait_for_active_requests(&handle, 0).await;

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_waiter_limit_sheds_load_instead_of_growing_unbounded() {
    const LIMIT: usize = 8;

    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_start_delay(Duration::from_millis(200));

    let handle = app::spawn_with_capacity(
        Arc::new(common::spec(60, LIMIT)),
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        512,
    );

    let mut callers = JoinSet::new();
    for _ in 0..128 {
        let handle = handle.clone();
        callers.spawn(async move { handle.acquire().await });
    }

    let mut accepted = 0usize;
    let mut overloaded = 0usize;
    let mut leases = Vec::new();

    while let Some(joined) = callers.join_next().await {
        match joined.expect("caller task did not panic") {
            Ok(lease) => {
                accepted += 1;
                leases.push(lease);
            }
            Err(AcquireError::Overloaded) => overloaded += 1,
            Err(other) => panic!("unexpected acquire error: {other}"),
        }
    }

    assert_eq!(accepted + overloaded, 128);
    // 상한을 넘은 요청은 적재하지 않고 즉시 거절한다. (Invariant 9 / PRD §69)
    assert!(
        accepted <= LIMIT,
        "accepted {accepted} waiters, limit is {LIMIT}"
    );
    assert!(overloaded > 0, "overload must shed load");
    assert_eq!(
        runtime.start_count(),
        1,
        "shedding must not spawn extra processes"
    );

    drop(leases);
    handle.shutdown().await;
}
