//! 실제 `nimwaked`를 띄우는 end-to-end 회귀 테스트.
//!
//! 요구사항: PRD §41 E2E Tests, §44 CI Strategy.
//! 마일스톤: `.docs/phases/phase-1/03-milestones/04-v0.0.4-e2e-performance-baseline.md`
//!
//! 여기까지 오면 in-process 조립이 아니라 **배포되는 바이너리**가 검증 대상이다.
//! CLI 파싱, tracing 초기화, signal 처리, 종료 순서가 모두 포함된다.
//!
//! 각 테스트는 자기 daemon을 `--port 0`으로 띄운다. 고정 port를 쓰면 서로 충돌한다.

#![cfg(unix)]

mod harness;

use std::time::Duration;

use harness::{fixture, free_port, is_alive, is_listening, wait_for, Apps, Daemon, Lifecycle};

fn echo(apps: &Apps, name: &str, port: u16, lifecycle: Lifecycle) {
    let command = [
        fixture("echo-server").display().to_string(),
        port.to_string(),
    ];
    apps.app(name, port, &command, lifecycle);
}

/// PRD §41의 primary regression 시나리오.
///
/// ```text
/// start nimwaked → backend process == 0 → 100 concurrent requests
///   → HTTP 200, unique backend PID == 1
///   → idle timeout → backend process == 0
///   → new request → 200, new PID != old PID
/// ```
#[test]
fn the_primary_regression_scenario() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "hello", port, Lifecycle::default().idle(1));
    let daemon = Daemon::start(&apps);

    assert!(!is_listening(port), "every application starts asleep");

    // 100개의 동시 요청이 하나의 cold start로 합쳐져야 한다. (PRD §16, §40)
    let gateway = daemon.port();
    let workers: Vec<_> = (0..100)
        .map(|_| std::thread::spawn(move || harness::get(gateway, "hello.localhost")))
        .collect();

    let responses: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("client thread finished"))
        .map(|result| result.expect("gateway responds"))
        .collect();

    assert!(
        responses.iter().all(|response| response.status == 200),
        "every request must succeed: {:?}",
        responses.iter().map(|r| r.status).collect::<Vec<_>>(),
    );

    let pids: std::collections::HashSet<_> = responses.iter().map(|r| r.pid()).collect();
    assert_eq!(
        pids.len(),
        1,
        "100 concurrent requests must start one process"
    );
    let first = *pids.iter().next().expect("one pid");

    wait_for(
        "the idle timeout to stop the app",
        Duration::from_secs(10),
        || !is_listening(port),
    );

    let again = daemon.get("hello.localhost");
    assert_eq!(again.status, 200);
    assert_ne!(
        again.pid(),
        first,
        "waking up again must start a new process"
    );
}

#[test]
fn a_warm_request_reuses_the_process() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "warm", port, Lifecycle::default());
    let daemon = Daemon::start(&apps);

    let first = daemon.get("warm.localhost");
    let second = daemon.get("warm.localhost");

    assert_eq!(first.status, 200);
    assert_eq!(second.status, 200);
    assert_eq!(first.pid(), second.pid());
}

/// 요청이 계속 오는 동안에는 idle timer가 매번 리셋된다. (PRD §29)
///
/// 여기서 process가 바뀐다면 stale timer가 현재 세대를 정지시킨 것이다.
#[test]
fn a_request_before_the_idle_deadline_keeps_the_app_alive() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "busy", port, Lifecycle::default().idle(1));
    let daemon = Daemon::start(&apps);

    let first = daemon.get("busy.localhost");

    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(400));
        let response = daemon.get("busy.localhost");
        assert_eq!(response.status, 200);
        assert_eq!(
            response.pid(),
            first.pid(),
            "a stale idle timer must not stop the current process",
        );
    }
}

#[test]
fn an_app_that_crashes_is_restarted_by_the_next_request() {
    let apps = Apps::new();
    let port = free_port();
    let command = [
        fixture("exit-after-server").display().to_string(),
        port.to_string(),
        "400".to_owned(),
    ];
    apps.app("flaky", port, &command, Lifecycle::default());
    let daemon = Daemon::start(&apps);

    let first = daemon.get("flaky.localhost");
    assert_eq!(first.status, 200);

    wait_for("the app to die on its own", Duration::from_secs(10), || {
        !is_listening(port)
    });

    let second = daemon.get("flaky.localhost");
    assert_eq!(second.status, 200, "a crashed app restarts on demand");
    assert_ne!(second.pid(), first.pid());
}

/// readiness가 startup timeout 안에 오지 않으면 `502`다. (PRD §22)
#[test]
fn an_app_that_never_becomes_ready_returns_502() {
    let apps = Apps::new();
    let port = free_port();
    let command = [
        fixture("slow-start-server").display().to_string(),
        port.to_string(),
        "5000".to_owned(),
    ];
    apps.app("slow", port, &command, Lifecycle::default().startup(1));
    let daemon = Daemon::start(&apps);

    let response = daemon.get("slow.localhost");
    assert_eq!(response.status, 502, "body: {}", response.body);

    // 취소된 기동이 child를 남기지 않았어야 한다. (Invariant 10)
    std::thread::sleep(Duration::from_millis(5500));
    assert!(!is_listening(port), "a cancelled cold start left an orphan");
}

/// STOPPING 중 도착한 요청은 실패시키지 않고 기다렸다가 재기동으로 처리한다. (PRD §31)
#[test]
fn a_request_during_stopping_is_served_after_the_restart() {
    let apps = Apps::new();
    let port = free_port();
    let command = [
        fixture("stubborn-server").display().to_string(),
        port.to_string(),
    ];
    apps.app(
        "stubborn",
        port,
        &command,
        Lifecycle::default().idle(1).shutdown(3),
    );
    let daemon = Daemon::start(&apps);

    let first = daemon.get("stubborn.localhost");
    assert_eq!(first.status, 200);

    // idle timeout이 지나 STOPPING에 들어가기를 기다린다.
    // stubborn-server는 SIGTERM을 무시하므로 shutdown timeout 동안 STOPPING이다.
    std::thread::sleep(Duration::from_millis(1500));

    let second = daemon.get("stubborn.localhost");
    assert_eq!(
        second.status, 200,
        "a request during STOPPING must not fail"
    );
    assert_ne!(
        second.pid(),
        first.pid(),
        "shutdown is not cancelled, so a new process serves the request",
    );
}

/// 상한을 넘은 요청은 `503`이고, process를 추가로 만들지 않는다. (PRD §69)
#[test]
fn requests_beyond_the_pending_limit_get_503() {
    const LIMIT: usize = 2;

    let apps = Apps::new();
    let port = free_port();
    let command = [
        fixture("slow-start-server").display().to_string(),
        port.to_string(),
        "1500".to_owned(),
    ];
    apps.app(
        "queue",
        port,
        &command,
        Lifecycle::default().max_pending(LIMIT),
    );
    let daemon = Daemon::start(&apps);

    let gateway = daemon.port();
    let workers: Vec<_> = (0..10)
        .map(|_| std::thread::spawn(move || harness::get(gateway, "queue.localhost")))
        .collect();

    let responses: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("client thread finished"))
        .map(|result| result.expect("gateway responds"))
        .collect();

    let ok: Vec<_> = responses.iter().filter(|r| r.status == 200).collect();
    let rejected = responses.iter().filter(|r| r.status == 503).count();

    assert!(!ok.is_empty(), "waiters within the limit must be served");
    assert!(rejected > 0, "requests beyond the limit must be rejected");
    assert!(
        ok.len() <= LIMIT,
        "at most {LIMIT} waiters may ride the cold start"
    );

    let pids: std::collections::HashSet<_> = ok.iter().map(|r| r.pid()).collect();
    assert_eq!(pids.len(), 1, "overload must not start extra processes");
}

/// stdout을 대량으로 쏟아도 child가 멈추지 않아야 한다. (PRD §19, §41)
#[test]
fn a_noisy_app_keeps_serving() {
    let apps = Apps::new();
    let port = free_port();
    let command = [
        fixture("noisy-server").display().to_string(),
        port.to_string(),
    ];
    apps.app("noisy", port, &command, Lifecycle::default());
    let daemon = Daemon::start(&apps);

    let first = daemon.get("noisy.localhost");
    assert_eq!(first.status, 200);

    // 기본 pipe buffer를 훨씬 넘길 만큼 출력하도록 시간을 준다.
    std::thread::sleep(Duration::from_millis(700));

    for attempt in 0..5 {
        let response = daemon.get("noisy.localhost");
        assert_eq!(response.status, 200, "attempt {attempt}");
        assert_eq!(
            response.pid(),
            first.pid(),
            "the process must not have restarted"
        );
    }
}

/// cold start를 거쳐도 POST body가 그대로 전달된다. (PRD §24)
#[test]
fn a_post_body_survives_a_cold_start() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "echo", port, Lifecycle::default());
    let daemon = Daemon::start(&apps);

    assert!(!is_listening(port), "the body must cross a real cold start");

    let payload = "nimwake end-to-end body";
    let response = daemon.post("echo.localhost", "/echo", payload);

    assert_eq!(response.status, 200);
    assert_eq!(response.body, payload);
}

/// SIGTERM을 무시하는 application이라도 shutdown timeout 뒤에는 반드시 정리된다. (PRD §30)
///
/// daemon이 정지 완료를 기다린다는 것까지 함께 본다. 기다리지 않으면 즉시 종료해 버린다.
#[test]
fn graceful_shutdown_waits_for_the_shutdown_timeout() {
    let apps = Apps::new();
    let port = free_port();
    let command = [
        fixture("stubborn-server").display().to_string(),
        port.to_string(),
    ];
    apps.app("stubborn", port, &command, Lifecycle::default().shutdown(2));
    let mut daemon = Daemon::start(&apps);

    let child = daemon.get("stubborn.localhost").pid();
    assert!(is_alive(child));

    let elapsed = daemon.stop();

    assert!(
        elapsed >= Duration::from_secs(2),
        "the daemon must wait for the shutdown timeout before SIGKILL, took {elapsed:?}",
    );
    assert!(!is_alive(child), "the child outlived the daemon");
}

/// child process의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10 / PRD §71)
#[test]
fn no_child_survives_the_daemon() {
    let apps = Apps::new();
    let ports = [free_port(), free_port()];
    echo(&apps, "one", ports[0], Lifecycle::default());
    echo(&apps, "two", ports[1], Lifecycle::default());
    let mut daemon = Daemon::start(&apps);

    let children = [
        daemon.get("one.localhost").pid(),
        daemon.get("two.localhost").pid(),
    ];
    assert!(children.iter().all(|pid| is_alive(*pid)));

    daemon.stop();

    for pid in children {
        assert!(!is_alive(pid), "child {pid} outlived the daemon");
    }
}

/// port가 충돌하는 manifest는 기동을 거부한다. (PRD §21)
#[test]
fn a_port_conflict_refuses_to_start() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "one", port, Lifecycle::default());
    echo(&apps, "two", port, Lifecycle::default());

    let (code, output) = Daemon::run_expecting_failure(&apps);

    assert_ne!(code, 0, "the daemon must refuse to start");
    assert!(
        output.contains("duplicate port"),
        "unexpected output: {output}",
    );
}

/// 등록되지 않은 host는 `404`이고 아무것도 깨우지 않는다. (PRD §23)
#[test]
fn an_unknown_host_wakes_nothing() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "known", port, Lifecycle::default());
    let daemon = Daemon::start(&apps);

    let response = daemon.get("unknown.localhost");

    assert_eq!(response.status, 404);
    assert!(!is_listening(port), "a 404 must not start anything");
}

/// benchmark mode는 구간 계측을 JSON 한 줄씩 내보낸다. (PRD §72)
#[test]
fn benchmark_mode_emits_span_measurements() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "traced", port, Lifecycle::default());
    let daemon = Daemon::start_with(&apps, &["--trace-json"]);

    assert_eq!(daemon.get("traced.localhost").status, 200);

    wait_for(
        "the trace events to be written",
        Duration::from_secs(5),
        || {
            let output = daemon.output();
            output.iter().any(|line| line.contains("nimwake::startup"))
                && output.iter().any(|line| line.contains("nimwake::request"))
        },
    );

    let output = daemon.output();
    let startup = output
        .iter()
        .find(|line| line.contains("nimwake::startup"))
        .expect("a startup trace is emitted");

    // cold start는 단일 숫자가 아니라 구간으로 나와야 한다. (PRD §42)
    for field in ["queue_us", "spawn_us", "readiness_us", "total_us"] {
        assert!(startup.contains(field), "`{field}` missing from: {startup}");
    }

    let request = output
        .iter()
        .find(|line| line.contains("nimwake::request"))
        .expect("a request trace is emitted");

    for field in ["acquire_us", "proxy_connect_us", "ttfb_us"] {
        assert!(request.contains(field), "`{field}` missing from: {request}");
    }
}
