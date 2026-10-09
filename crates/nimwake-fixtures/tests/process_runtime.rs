//! `ProcessRuntime` adapter 자체의 계약.
//!
//! 요구사항: PRD §19, §22, §30, §32, §70–§71.
//! 마일스톤: `.docs/phases/phase-1/03-milestones/02-v0.0.2-process-runtime.md`
//!
//! 여기서 검증하는 것은 runtime 한 겹이다. state machine과의 결합은
//! `process_lifecycle.rs`가 본다.

#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::{bin, cmd, free_port, http_get, is_alive, is_listening, spec, wait_for};
use nimwake_core::runtime::{ProcessRuntime, Runtime};

const STOP_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::test]
async fn start_waits_for_readiness_and_stop_removes_the_process() {
    let port = free_port();
    let spec = spec("echo", port, cmd(bin::ECHO, &[port.to_string()]));
    let runtime = ProcessRuntime::new();

    let started = runtime.start(&spec).await.expect("echo-server starts");

    // start가 반환됐다는 것은 readiness가 끝났다는 뜻이다. 추가로 기다릴 필요가 없다. (PRD §22)
    assert_eq!(started.backend.addr().port(), port);
    assert!(is_listening(port), "backend must accept connections");
    assert!(http_get(port).expect("request succeeds").contains("200"));

    runtime
        .stop(&started.handle, STOP_TIMEOUT)
        .await
        .expect("stop succeeds");

    assert!(!is_listening(port), "port must be released after stop");
}

#[tokio::test]
async fn start_fails_when_the_process_exits_before_readiness() {
    let port = free_port();
    let spec = spec("crash", port, cmd(bin::CRASH, &[]));
    let runtime = ProcessRuntime::new();

    let error = runtime
        .start(&spec)
        .await
        .expect_err("crash-server never becomes ready");

    // readiness timeout이 아니라 종료를 근거로 즉시 실패해야 한다. (PRD §22)
    assert!(
        error.to_string().contains("exited during startup"),
        "unexpected error: {error}",
    );
}

#[tokio::test]
async fn start_fails_when_the_command_does_not_exist() {
    let port = free_port();
    let spec = spec("missing", port, cmd("./definitely-not-a-real-binary", &[]));
    let runtime = ProcessRuntime::new();

    let error = runtime.start(&spec).await.expect_err("spawn fails");
    assert!(
        error.to_string().contains("failed to spawn"),
        "unexpected error: {error}",
    );
}

/// startup timeout은 호출자가 `start`를 취소하는 방식으로 적용된다.
/// 취소된 cold start가 child를 남기면 orphan이 쌓인다. (Invariant 10 / PRD §71)
#[tokio::test]
async fn cancelling_start_terminates_the_spawned_process() {
    let port = free_port();
    let spec = spec(
        "slow",
        port,
        cmd(bin::SLOW_START, &[port.to_string(), "3000".to_owned()]),
    );
    let runtime = ProcessRuntime::new();

    let outcome = tokio::time::timeout(Duration::from_millis(200), runtime.start(&spec)).await;
    assert!(
        outcome.is_err(),
        "start must still be pending when cancelled"
    );

    // 살아남았다면 3초 뒤 port를 열 것이다. 열리지 않는다는 것이 정리됐다는 증거다.
    tokio::time::sleep(Duration::from_millis(3500)).await;
    assert!(
        !is_listening(port),
        "cancelled start left an orphan process listening on {port}",
    );
}

/// SIGTERM을 무시하는 application은 shutdown timeout 뒤 SIGKILL로 정리한다. (PRD §30)
#[tokio::test]
async fn sigterm_is_escalated_to_sigkill() {
    let port = free_port();
    let spec = spec("stubborn", port, cmd(bin::STUBBORN, &[port.to_string()]));
    let runtime = ProcessRuntime::new();

    let started = runtime.start(&spec).await.expect("stubborn-server starts");

    let shutdown_timeout = Duration::from_millis(500);
    let began = Instant::now();
    runtime
        .stop(&started.handle, shutdown_timeout)
        .await
        .expect("stop succeeds even when SIGTERM is ignored");
    let elapsed = began.elapsed();

    assert!(
        elapsed >= shutdown_timeout,
        "SIGKILL must come after the shutdown timeout, took {elapsed:?}",
    );
    assert!(!is_listening(port), "process survived SIGKILL");
}

/// 종료 신호는 process group 단위로 간다. child가 만든 손자도 함께 사라져야 한다. (PRD §71)
#[tokio::test]
async fn stop_terminates_the_whole_process_group() {
    let port = free_port();
    let pid_file = std::env::temp_dir().join(format!("nimwake-group-{port}.pid"));
    let _ = std::fs::remove_file(&pid_file);

    let spec = spec(
        "group",
        port,
        cmd(
            bin::GROUP,
            &[port.to_string(), pid_file.display().to_string()],
        ),
    );
    let runtime = ProcessRuntime::new();

    let started = runtime.start(&spec).await.expect("group-server starts");

    let grandchild: u32 = std::fs::read_to_string(&pid_file)
        .expect("fixture wrote its grandchild pid")
        .trim()
        .parse()
        .expect("pid file holds a pid");
    assert!(is_alive(grandchild), "grandchild should be running");

    runtime
        .stop(&started.handle, STOP_TIMEOUT)
        .await
        .expect("stop succeeds");

    wait_for(
        "the grandchild to be reaped",
        Duration::from_secs(5),
        || !is_alive(grandchild),
    )
    .await;

    let _ = std::fs::remove_file(&pid_file);
}

/// pipe를 방치하면 buffer가 차는 순간 child가 멈춘다. drain하고 있다면 계속 응답한다. (PRD §19)
///
/// **두 가지를 동시에 맞춰야 이 테스트가 의미를 갖는다.**
///
/// 하나는 fixture가 **응답 경로에서** 쓰는 것이다. 전에는 별도 스레드에서 썼고,
/// 그러면 pipe가 차도 그 스레드만 멈추므로 main은 계속 응답했다. drain을
/// 제거해도 통과했다.
///
/// 다른 하나는 이 테스트가 runtime을 막지 않는 것이다. drain은 `tokio::spawn`으로
/// 돌고, HTTP client는 blocking이다. 단일 스레드 runtime에서 client를 직접 부르면
/// **drain이 굶어** pipe가 차고, drain이 있는데도 실패한다.
/// gateway 테스트가 같은 이유로 이미 `spawn_blocking`을 쓰고 있다.
#[tokio::test(flavor = "multi_thread")]
async fn a_noisy_process_keeps_serving() {
    let port = free_port();
    let spec = spec("noisy", port, cmd(bin::NOISY, &[port.to_string()]));
    let runtime = ProcessRuntime::new();

    let started = runtime.start(&spec).await.expect("noisy-server starts");

    // 요청마다 128KiB를 쏟아 낸다. 다섯 번이면 기본 pipe buffer를 열 배 넘긴다.
    for attempt in 0..5 {
        let response = tokio::task::spawn_blocking(move || http_get(port))
            .await
            .expect("client task finished")
            .unwrap_or_else(|error| {
                panic!("noisy-server stopped responding on attempt {attempt}: {error}")
            });
        assert!(response.contains("200"), "unexpected response: {response}");
    }

    runtime
        .stop(&started.handle, STOP_TIMEOUT)
        .await
        .expect("stop succeeds");
}

#[tokio::test]
async fn wait_reports_the_exit_status_of_a_process_that_dies_on_its_own() {
    let port = free_port();
    let spec = spec(
        "exiting",
        port,
        cmd(bin::EXIT_AFTER, &[port.to_string(), "200".to_owned()]),
    );
    let runtime = ProcessRuntime::new();

    let started = runtime
        .start(&spec)
        .await
        .expect("exit-after-server starts");

    let status = tokio::time::timeout(Duration::from_secs(5), runtime.wait(&started.handle))
        .await
        .expect("process exits within the deadline");

    assert_eq!(status.code, Some(17), "fixture exits with 17");
    assert!(!status.success());
}

/// 이미 끝난 process에 대한 `wait`는 즉시 반환해야 한다. (PRD §18)
#[tokio::test]
async fn wait_returns_immediately_for_an_already_stopped_process() {
    let port = free_port();
    let spec = spec("echo", port, cmd(bin::ECHO, &[port.to_string()]));
    let runtime = ProcessRuntime::new();

    let started = runtime.start(&spec).await.expect("echo-server starts");
    runtime
        .stop(&started.handle, STOP_TIMEOUT)
        .await
        .expect("stop succeeds");

    tokio::time::timeout(Duration::from_millis(200), runtime.wait(&started.handle))
        .await
        .expect("wait on a dead process must not block");
}

/// 같은 spec을 반복해서 기동·종료해도 남는 것이 없어야 한다.
#[tokio::test]
async fn repeated_start_and_stop_leaves_nothing_behind() {
    let port = free_port();
    let spec = spec("echo", port, cmd(bin::ECHO, &[port.to_string()]));
    let runtime = ProcessRuntime::new();

    let mut process_ids = Vec::new();

    for round in 0..5 {
        let started = runtime
            .start(&spec)
            .await
            .unwrap_or_else(|error| panic!("round {round} failed to start: {error}"));
        process_ids.push(started.handle.id());

        runtime
            .stop(&started.handle, STOP_TIMEOUT)
            .await
            .expect("stop succeeds");

        // 다음 회차가 같은 port를 다시 bind할 수 있어야 한다.
        assert!(!is_listening(port), "round {round} left the port open");
    }

    process_ids.dedup();
    assert_eq!(
        process_ids.len(),
        5,
        "each start must get a fresh ProcessId"
    );
}
