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

use common::{bin, cmd, free_port, is_listening, request, spec, with_runtime, with_timeouts};
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

/// 목표 상태에 도달할 때까지 기다린다.
///
/// **port가 닫히기를 기다린 뒤 state를 단언하면 안 된다.** port는 STOPPING 중에 닫히고,
/// SLEEPING은 그 완료 event가 mailbox를 한 바퀴 돈 뒤다. 그 사이에 단언하면 간헐적으로 깨진다.
/// SLEEPING은 process가 사라졌음을 함의하지만 그 역은 아니다.
async fn wait_for_state(app: &AppHandle, target: AppStateName, deadline: Duration) {
    let started = std::time::Instant::now();
    while started.elapsed() < deadline {
        if state(app).await == target {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never reached {target}, still {}", state(app).await);
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

    wait_for_state(&app, AppStateName::Sleeping, Duration::from_secs(5)).await;
    assert!(!is_listening(port), "SLEEPING이면 process가 없다");

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

    // 기다리지 않고 곧바로 확인한다.
    //
    // `shutdown()`이 정지 요청만 보내고 반환하면 여기서 통과해 버리는 테스트가 되는데,
    // 실제 daemon은 반환 직후 종료하므로 stop task가 끝나지 못하고 child가 남는다.
    // 즉 "완료를 기다렸는가"까지 검증하려면 유예를 주면 안 된다.
    for port in ports {
        assert!(
            !is_listening(port),
            "shutdown() returned before port {port} was released",
        );
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

    wait_for_state(&app, AppStateName::Sleeping, Duration::from_secs(8)).await;
    assert!(!is_listening(port), "SLEEPING이면 process가 없다");
    app.shutdown().await;
}

/// §20은 app별 분리를 두 가지로 적는다. 둘 다 밖에서 관측해 고정한다.
///
/// ```text
/// app별 working directory 분리
/// app별 environment / secret 분리 (`[runtime.env]`, daemon 환경 위에 overlay)
/// ```
///
/// **"overlay"가 핵심이다.** 덮어쓰기로 구현하면 app이 지정하지 않은 변수가
/// 사라진다. `PATH`가 없으면 대부분의 런타임이 기동하지 못한다. 격리를 강화한다는
/// 이유로 `.env_clear()`를 넣기 쉬운 자리이고, 그러면 조용히 깨진다.
#[tokio::test(flavor = "multi_thread")]
async fn the_child_gets_its_own_directory_and_an_overlaid_environment() {
    // daemon(= 이 테스트 프로세스) 환경에만 있는 변수.
    let inherited_key = "NIMWAKE_TEST_INHERITED";
    let inherited_value = format!("daemon-{}", std::process::id());
    std::env::set_var(inherited_key, &inherited_value);

    // 양쪽에 있는 변수. app 쪽이 이겨야 한다.
    let overridden_key = "NIMWAKE_TEST_OVERRIDDEN";
    std::env::set_var(overridden_key, "from-daemon");

    let dir = std::env::temp_dir().join(format!("nimwake-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("the app directory exists");

    let port = free_port();
    let spec = with_runtime(
        spec("isolated", port, cmd(bin::ECHO, &[port.to_string()])),
        dir.clone(),
        &[
            ("NIMWAKE_TEST_OWN", "from-app"),
            (overridden_key, "from-app"),
        ],
    );

    let app = start_actor(with_timeouts(spec, 5, 2, 60));
    let _lease = app.acquire().await.expect("cold start succeeds");

    let ask = |path: &str| {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || {
            request(port, "GET", &path, "isolated.localhost", None)
                .expect("the fixture answers")
                .body
        })
    };

    // app이 지정한 변수.
    assert_eq!(ask("/env/NIMWAKE_TEST_OWN").await.unwrap(), "from-app");

    // daemon 환경이 **그대로 넘어온다.** 덮어쓰기였다면 비어 있다.
    assert_eq!(
        ask(&format!("/env/{inherited_key}")).await.unwrap(),
        inherited_value,
        "`[runtime.env]`는 daemon 환경을 덮지 않고 올려 쓴다",
    );

    // 같은 키는 app이 이긴다.
    assert_eq!(
        ask(&format!("/env/{overridden_key}")).await.unwrap(),
        "from-app",
        "app이 지정한 값이 daemon 값을 가린다",
    );

    // 작업 디렉터리가 app의 것이다.
    let cwd = ask("/cwd").await.unwrap();
    let expected = dir.canonicalize().unwrap_or(dir.clone());
    assert_eq!(
        std::path::Path::new(&cwd)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&cwd)),
        expected,
        "child must run in the app's working_dir",
    );

    drop(_lease);
    let _ = std::fs::remove_dir_all(&dir);
}
