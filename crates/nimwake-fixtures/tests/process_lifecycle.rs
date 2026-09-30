//! 실제 process 위에서 본 lifecycle state machine.
//!
//! 요구사항: PRD §22, §30, §32–§33, §71.
//! 마일스톤: `.docs/phases/phase-1/03-milestones/02-v0.0.2-process-runtime.md`
//!
//! 이 마일스톤의 증명 대상은 **runtime adapter를 바꿔도 lifecycle semantics가 같다**는 것이다.
//! 따라서 여기 있는 시나리오는 `FakeRuntime`으로 이미 검증한 것과 같은 전이를 본다.

#![cfg(unix)]

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{bin, cmd, free_port, is_listening, spec, wait_for, with_timeouts};
use nimwake_core::app::{AppSpec, AppStateName, StopReason};
use nimwake_core::error::{AcquireError, AppError};
use nimwake_core::runtime::ProcessRuntime;
use nimwake_core::{AppHandle, AppRegistry, DEFAULT_GATEWAY_PORT};

fn start_actor(spec: AppSpec) -> AppHandle {
    nimwake_core::app::spawn(Arc::new(spec), Arc::new(ProcessRuntime::new()))
}

async fn state(app: &AppHandle) -> AppStateName {
    app.status().await.expect("actor is alive").state
}

/// SLEEPING → spawn → ready → RUNNING → stop → process gone → SLEEPING.
///
/// 마일스톤 Acceptance 그 자체다.
#[tokio::test]
async fn sleeping_to_running_to_sleeping_on_a_real_process() {
    let port = free_port();
    let app = start_actor(with_timeouts(
        spec("echo", port, cmd(bin::ECHO, &[port.to_string()])),
        5,
        2,
        60,
    ));

    assert_eq!(state(&app).await, AppStateName::Sleeping);
    assert!(!is_listening(port));

    let lease = app.acquire().await.expect("cold start succeeds");

    let status = app.status().await.expect("actor is alive");
    assert_eq!(status.state, AppStateName::Running);
    assert_eq!(status.active_requests, 1);
    assert!(status.process_id.is_some());
    assert!(is_listening(port), "backend must be reachable once RUNNING");

    // lease를 반납해야 stop이 진행될 수 있다.
    drop(lease);
    app.stop(StopReason::Requested)
        .await
        .expect("stop accepted");

    wait_for(
        "the app to go back to SLEEPING",
        Duration::from_secs(5),
        || !is_listening(port),
    )
    .await;

    let status = app.status().await.expect("actor is alive");
    assert_eq!(status.state, AppStateName::Sleeping);
    assert_eq!(status.active_requests, 0);
    assert!(status.process_id.is_none());

    app.shutdown().await;
}

/// readiness가 startup timeout 안에 오지 않으면 FAILED다.
/// 그리고 취소된 기동이 child를 남기지 않아야 한다. (PRD §22, Invariant 10)
#[tokio::test]
async fn startup_timeout_moves_to_failed_and_leaves_no_process() {
    let port = free_port();
    let app = start_actor(with_timeouts(
        spec(
            "slow",
            port,
            cmd(bin::SLOW_START, &[port.to_string(), "4000".to_owned()]),
        ),
        1,
        2,
        60,
    ));

    let error = app.acquire().await.expect_err("startup times out");
    assert!(
        matches!(
            error,
            AcquireError::StartFailed(AppError::StartupTimeout(_))
        ),
        "unexpected error: {error:?}",
    );
    assert_eq!(state(&app).await, AppStateName::Failed);

    // fixture가 살아 있었다면 4초 뒤 port를 열었을 것이다.
    tokio::time::sleep(Duration::from_millis(4500)).await;
    assert!(
        !is_listening(port),
        "a cancelled cold start left an orphan behind",
    );

    app.shutdown().await;
}

/// RUNNING 중 process가 죽으면 FAILED로 가고, 다음 `acquire`가 다시 기동한다. (PRD §32–§33)
#[tokio::test]
async fn a_process_that_dies_while_running_fails_then_restarts() {
    let port = free_port();
    let app = start_actor(with_timeouts(
        spec(
            "flaky",
            port,
            cmd(bin::EXIT_AFTER, &[port.to_string(), "400".to_owned()]),
        ),
        5,
        2,
        60,
    ));

    let lease = app.acquire().await.expect("first start succeeds");
    let first = app.status().await.expect("actor is alive").process_id;
    assert!(first.is_some());
    drop(lease);

    // fixture가 스스로 죽는다. monitor가 ProcessExited를 보고해야 한다.
    let app_for_wait = app.clone();
    let reached_failed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if state(&app_for_wait).await == AppStateName::Failed {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(reached_failed.is_ok(), "app never reached FAILED");

    let status = app.status().await.expect("actor is alive");
    assert!(status.last_error.is_some(), "FAILED must record why");
    assert!(status.process_id.is_none());

    // FAILED에서 새 요청이 오면 다시 기동한다.
    let lease = app.acquire().await.expect("restart from FAILED succeeds");
    let second = app.status().await.expect("actor is alive").process_id;
    assert!(second.is_some());
    assert_ne!(first, second, "restart must use a fresh ProcessId");

    drop(lease);
    app.shutdown().await;
}

/// child의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10 / PRD §71)
#[tokio::test]
async fn registry_shutdown_leaves_no_managed_child() {
    let ports = [free_port(), free_port()];
    let specs = vec![
        with_timeouts(
            spec("one", ports[0], cmd(bin::ECHO, &[ports[0].to_string()])),
            5,
            2,
            60,
        ),
        with_timeouts(
            spec("two", ports[1], cmd(bin::ECHO, &[ports[1].to_string()])),
            5,
            2,
            60,
        ),
    ];

    let registry = AppRegistry::build(specs, DEFAULT_GATEWAY_PORT, Arc::new(ProcessRuntime::new()))
        .expect("specs validate");

    for name in ["one", "two"] {
        let app = registry.get(name).expect("app is registered");
        let lease = app.acquire().await.expect("cold start succeeds");
        drop(lease);
    }
    assert!(ports.iter().all(|port| is_listening(*port)));

    registry.shutdown().await;

    for port in ports {
        wait_for(
            &format!("port {port} to be released"),
            Duration::from_secs(5),
            || !is_listening(port),
        )
        .await;
    }
}

/// idle timeout이 지나면 요청이 없는 application은 스스로 잠든다. (PRD §30)
#[tokio::test]
async fn an_idle_application_stops_its_process() {
    let port = free_port();
    let app = start_actor(with_timeouts(
        spec("idle", port, cmd(bin::ECHO, &[port.to_string()])),
        5,
        2,
        1,
    ));

    let lease = app.acquire().await.expect("cold start succeeds");
    assert!(is_listening(port));

    // lease가 살아 있는 동안에는 idle timer가 돌지 않는다. (PRD §25)
    drop(lease);

    wait_for(
        "the idle timeout to stop the process",
        Duration::from_secs(8),
        || !is_listening(port),
    )
    .await;

    assert_eq!(state(&app).await, AppStateName::Sleeping);
    app.shutdown().await;
}
