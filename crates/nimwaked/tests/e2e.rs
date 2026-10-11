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

    // 이름만 보면 안 된다. 한때 계측값을 `u128`로 두었더니 tracing이 JSON에
    // **문자열**로 적었고, 이름만 보는 단언은 그것을 통과시켰다. (D29)
    //
    // 숫자로 뽑아 구간 관계까지 본다.
    let spans = numeric_fields(
        request,
        &[
            "routing_us",
            "acquire_us",
            "proxy_connect_us",
            "backend_us",
            "ttfb_us",
        ],
    );

    // proxy connect와 backend는 나뉘어 있어야 한다. (PRD §72)
    //
    // 한동안 `proxy_connect_us`가 backend 왕복 전체를 담고 있었다. 둘을 합쳐
    // 두면 "연결이 느린가 / application이 느린가"를 구분할 수 없다.
    let connect = spans["proxy_connect_us"];
    let backend = spans["backend_us"];
    let ttfb = spans["ttfb_us"];

    assert!(
        connect + backend <= ttfb,
        "connect({connect}) + backend({backend}) must fit in ttfb({ttfb}): {request}",
    );
    assert!(
        spans["routing_us"] + spans["acquire_us"] + connect + backend <= ttfb,
        "the spans must be consecutive sub-intervals of ttfb: {request}",
    );
}

/// JSON 한 줄에서 숫자 필드를 뽑는다. 문자열로 적혔으면 panic한다.
///
/// `"queue_us":142`는 통과하고 `"queue_us":"142"`는 실패한다. 후자는 실제로
/// 겪은 일이다. 계측값을 `u128`로 두면 tracing이 그렇게 적는다. (D29)
fn numeric_fields<'a>(line: &str, names: &[&'a str]) -> std::collections::BTreeMap<&'a str, u64> {
    names
        .iter()
        .map(|name| {
            let needle = format!("\"{name}\":");
            let start = line
                .find(&needle)
                .unwrap_or_else(|| panic!("`{name}` missing from: {line}"))
                + needle.len();
            let rest = &line[start..];

            assert!(
                !rest.starts_with('"'),
                "`{name}` is a string, not a number: {line}",
            );

            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let value = digits
                .parse()
                .unwrap_or_else(|_| panic!("`{name}` is not a number: {line}"));
            (*name, value)
        })
        .collect()
}

/// 요청 구간은 요청마다 나가므로 평소에는 꺼져 있어야 한다.
///
/// 켜 두면 운영 중인 daemon이 요청 하나당 로그 한 줄을 남긴다.
/// 비용도 공짜가 아니다 — 켜져 있으면 요청당 약 2.5µs, 꺼져 있으면 약 0.3ns다.
#[test]
fn request_traces_are_off_unless_asked_for() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "quiet", port, Lifecycle::default());
    let daemon = Daemon::start(&apps);

    assert_eq!(daemon.get("quiet.localhost").status, 200);

    // 기동은 드물게 일어나고 운영자가 보고 싶어 하므로 평소에도 남는다.
    wait_for("the startup trace", Duration::from_secs(5), || {
        daemon
            .output()
            .iter()
            .any(|line| line.contains("nimwake::startup"))
    });

    assert!(
        !daemon
            .output()
            .iter()
            .any(|line| line.contains("nimwake::request")),
        "per-request traces must stay off by default",
    );
}

// 아래는 supervisor가 의존하는 외부 동작이다. 바뀌면 host 쪽이 조용히 깨진다. (PRD §74)

/// 설정 오류는 `78`이다. supervisor는 재기동하지 않고 operator에게 넘긴다.
#[test]
fn a_configuration_failure_exits_with_78() {
    let apps = Apps::new();

    // manifest가 하나도 없다.
    let (code, output) = Daemon::run_expecting_failure(&apps);
    assert_eq!(code, 78, "unexpected output: {output}");
    assert!(output.contains("no manifests found"), "{output}");

    // host/port가 충돌한다.
    let port = free_port();
    echo(&apps, "one", port, Lifecycle::default());
    echo(&apps, "two", port, Lifecycle::default());
    let (code, output) = Daemon::run_expecting_failure(&apps);
    assert_eq!(code, 78, "unexpected output: {output}");
    assert!(output.contains("duplicate port"), "{output}");
}

/// 잘못된 인자도 설정 오류다.
#[test]
fn a_bad_argument_exits_with_78() {
    let (code, output) = Daemon::run(&["--nonsense"]);
    assert_eq!(code, 78, "unexpected output: {output}");
    assert!(output.contains("unknown option"), "{output}");
}

/// bind 실패는 `69`다. 시간이 해결할 수 있으므로 supervisor는 backoff 후 재시도한다.
#[test]
fn a_bind_failure_exits_with_69() {
    let apps = Apps::new();
    let app_port = free_port();
    echo(&apps, "taken", app_port, Lifecycle::default());

    // 먼저 뜬 daemon이 port를 쥐고 있다.
    let first = Daemon::start(&apps);
    let gateway = first.port();

    let (code, output) = Daemon::run(&[
        "--apps",
        &apps.path().display().to_string(),
        "--port",
        &gateway.to_string(),
    ]);

    assert_eq!(code, 69, "unexpected output: {output}");
    assert!(output.contains("failed to bind"), "{output}");
}

/// 기본은 loopback이다. 외부 노출은 opt-in이어야 한다. (PRD §74, §46)
#[test]
fn the_gateway_stays_on_loopback_unless_asked() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "local", port, Lifecycle::default());

    let daemon = Daemon::start(&apps);
    assert!(
        daemon.listen_addr().starts_with("127.0.0.1:"),
        "unexpected listen address: {}",
        daemon.listen_addr(),
    );
}

/// `--listen`으로 요청하면 loopback 밖으로 연다.
#[test]
fn the_gateway_opens_beyond_loopback_when_asked() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "lan", port, Lifecycle::default());

    let daemon = Daemon::start_with(&apps, &["--listen", "0.0.0.0:0"]);

    assert!(
        daemon.listen_addr().starts_with("0.0.0.0:"),
        "unexpected listen address: {}",
        daemon.listen_addr(),
    );
    // loopback으로도 여전히 닿는다.
    assert_eq!(daemon.get("lan.localhost").status, 200);
}

/// health probe는 어떤 application의 lifecycle도 건드리지 않아야 한다. (PRD §74)
///
/// supervisor가 등록된 Host로 probe하면 그 요청이 `acquire()`를 거쳐 app을 깨우고
/// idle timer를 계속 리셋한다. **supervisor가 scale-to-zero를 무력화한다.**
/// 그래서 "미등록 Host 요청은 상태를 바꾸지 않는다"를 계약으로 고정한다.
#[test]
fn health_probes_do_not_keep_an_app_awake() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "sleepy", port, Lifecycle::default().idle(1));
    let daemon = Daemon::start(&apps);

    let awake = daemon.get("sleepy.localhost");
    assert_eq!(awake.status, 200);
    assert!(is_listening(port));

    // supervisor가 하듯 짧은 간격으로 계속 probe한다.
    // 이것이 idle timer를 건드린다면 app은 영원히 잠들지 못한다.
    let probes = std::thread::spawn({
        let gateway = daemon.port();
        move || {
            let mut seen = Vec::new();
            for _ in 0..20 {
                seen.push(harness::get(gateway, "nimwake-health.invalid").map(|r| r.status));
                std::thread::sleep(Duration::from_millis(200));
            }
            seen
        }
    });

    wait_for(
        "the app to sleep while health probes keep arriving",
        Duration::from_secs(6),
        || !is_listening(port),
    );

    let statuses = probes.join().expect("probe thread finished");
    assert!(
        statuses.iter().all(|status| matches!(status, Ok(404))),
        "every probe must be answered with 404: {statuses:?}",
    );
}

/// probe는 잠든 app을 깨우지도 않는다.
#[test]
fn health_probes_do_not_wake_a_sleeping_app() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "asleep", port, Lifecycle::default());
    let daemon = Daemon::start(&apps);

    for _ in 0..10 {
        assert_eq!(daemon.get("nimwake-health.invalid").status, 404);
    }

    assert!(!is_listening(port), "a health probe woke an application");
}

// 아래는 fixture가 아니라 **실제 애플리케이션**으로 도는 경로다.
//
// 지금까지의 모든 검증은 133줄짜리 Rust fixture 위에서 돌았다. 그것으로는
// "node 같은 런타임이 올라오는 시간", "앱이 SIGTERM에 스스로 나가는가",
// "앱 하나가 차지하는 메모리"를 알 수 없다.
//
// node가 없는 환경에서는 건너뛴다. 예제를 CI에서만 강제하면 로컬 실행이 막힌다.

/// `node` 실행 파일. `NIMWAKE_NODE`로 지정할 수 있다.
fn node() -> Option<std::path::PathBuf> {
    if let Ok(explicit) = std::env::var("NIMWAKE_NODE") {
        let path = std::path::PathBuf::from(explicit);
        return path.is_file().then_some(path);
    }

    let output = std::process::Command::new("sh")
        .args(["-c", "command -v node"])
        .output()
        .ok()?;

    let path = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    path.is_file().then_some(path)
}

/// 저장소의 `examples/hello-node`.
fn hello_node_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/hello-node")
        .canonicalize()
        .expect("the example directory exists")
}

fn hello_node_app(apps: &Apps, port: u16, node: &std::path::Path) {
    apps.manifest(
        "hello",
        &format!(
            "name = \"hello\"\n\
             \n\
             [server]\n\
             host = \"hello.localhost\"\n\
             port = {port}\n\
             \n\
             [runtime]\n\
             command = [{:?}, \"server.js\"]\n\
             working_dir = {:?}\n\
             \n\
             [runtime.env]\n\
             PORT = \"{port}\"\n\
             \n\
             [lifecycle]\n\
             idle_timeout_seconds = 1\n\
             startup_timeout_seconds = 30\n\
             shutdown_timeout_seconds = 5\n",
            node.display().to_string(),
            hello_node_dir().display().to_string(),
        ),
    );
}

/// `examples/hello-node`가 실제로 Nimwake를 통해 서빙되는지 본다.
///
/// 이 테스트가 없으면 예제는 저장소에 있지만 아무도 실행하지 않는 코드가 된다.
/// 실제로 `v0.0.5`까지 그랬다.
#[test]
fn the_node_example_serves_through_nimwake() {
    let Some(node) = node() else {
        eprintln!("skipped: `node` not found (set NIMWAKE_NODE to point at it)");
        return;
    };

    let apps = Apps::new();
    let port = free_port();
    hello_node_app(&apps, port, &node);
    let daemon = Daemon::start_with(&apps, &["--trace-json"]);

    assert!(!is_listening(port), "the example starts asleep");

    let first = daemon.get("hello.localhost");
    assert_eq!(first.status, 200, "body: {}", first.body);
    assert!(
        first.body.contains("hello from nimwake"),
        "unexpected body: {}",
        first.body,
    );

    // 같은 process를 재사용한다.
    let second = daemon.get("hello.localhost");
    assert_eq!(
        second.body, first.body,
        "a warm request must reuse the process"
    );

    // idle timeout으로 잠들고, 다시 깨우면 새 process다.
    wait_for("the example to sleep", Duration::from_secs(10), || {
        !is_listening(port)
    });
    let third = daemon.get("hello.localhost");
    assert_eq!(third.status, 200);
    assert_ne!(
        third.body, first.body,
        "waking up again must start a new process"
    );

    // 실제 런타임의 기동 구간이 기록되어야 한다. (PRD §42)
    wait_for("the startup trace", Duration::from_secs(5), || {
        daemon
            .output()
            .iter()
            .any(|line| line.contains("nimwake::startup"))
    });
}

/// 실제 앱이 `SIGTERM`에 스스로 나가는지 본다.
///
/// fixture는 기본 signal 동작에 맡기지만, 실제 앱은 handler를 둔다.
/// daemon 종료가 shutdown timeout을 다 쓰지 않아야 정상이다.
#[test]
fn the_node_example_shuts_down_gracefully() {
    let Some(node) = node() else {
        eprintln!("skipped: `node` not found (set NIMWAKE_NODE to point at it)");
        return;
    };

    let apps = Apps::new();
    let port = free_port();
    hello_node_app(&apps, port, &node);
    let mut daemon = Daemon::start(&apps);

    assert_eq!(daemon.get("hello.localhost").status, 200);
    assert!(is_listening(port));

    let elapsed = daemon.stop();

    assert!(
        elapsed < Duration::from_secs(5),
        "the app should exit on SIGTERM without waiting for the shutdown timeout, took {elapsed:?}",
    );
    assert!(!is_listening(port), "the app outlived the daemon");
}

/// `SIGHUP`으로 application을 추가해도 **돌고 있던 것은 건드리지 않는다.** (PRD §76)
///
/// 이 테스트가 §76의 존재 이유다. 건드린다면 daemon 재시작과 다를 바 없다.
///
/// "건드리지 않았다"를 app process의 pid로 본다. `echo-server`가 자기 pid를
/// body로 돌려주므로 재적재 전후를 HTTP로 직접 비교할 수 있다.
#[test]
fn a_reload_adds_an_app_without_touching_the_running_ones() {
    let apps = Apps::new();
    let blog_port = free_port();
    let api_port = free_port();

    echo(
        &apps,
        "blog",
        blog_port,
        Lifecycle {
            idle: 300,
            ..Lifecycle::default()
        },
    );

    let daemon = Daemon::start(&apps);

    // blog를 깨워 두고 pid를 기억한다.
    let before = daemon.get("blog.localhost");
    assert_eq!(before.status, 200);
    let blog_pid = before.body.clone();
    assert!(!blog_pid.is_empty(), "echo-server reports its pid");

    // 아직 등록되지 않은 host는 404다.
    assert_eq!(
        daemon.get("api.localhost").status,
        404,
        "api is not registered yet",
    );

    // manifest를 더하고 재적재한다.
    echo(
        &apps,
        "api",
        api_port,
        Lifecycle {
            idle: 300,
            ..Lifecycle::default()
        },
    );
    daemon.reload();

    wait_for("the reload to report", Duration::from_secs(10), || {
        daemon
            .output()
            .iter()
            .any(|line| line.contains("reloaded:"))
    });

    // 새 app이 서빙한다.
    let api = daemon.get("api.localhost");
    assert_eq!(api.status, 200, "the new app must serve after a reload");

    // **blog는 같은 process다.**
    let after = daemon.get("blog.localhost");
    assert_eq!(after.status, 200);
    assert_eq!(
        after.body, blog_pid,
        "the running app must not be restarted by a reload",
    );

    assert!(
        is_listening(blog_port),
        "blog must still be listening on its own port",
    );
}

/// manifest에서 빼고 재적재하면 정지되고 routing에서 빠진다. (PRD §76)
#[test]
fn a_reload_stops_an_app_that_left_the_manifest() {
    let apps = Apps::new();
    let keep_port = free_port();
    let drop_port = free_port();

    for (name, port) in [("keep", keep_port), ("gone", drop_port)] {
        echo(
            &apps,
            name,
            port,
            Lifecycle {
                idle: 300,
                ..Lifecycle::default()
            },
        );
    }

    let daemon = Daemon::start(&apps);

    // 둘 다 깨워 둔다.
    assert_eq!(daemon.get("keep.localhost").status, 200);
    assert_eq!(daemon.get("gone.localhost").status, 200);
    assert!(is_listening(drop_port));

    // manifest를 지우고 재적재한다.
    std::fs::remove_dir_all(apps.path().join("gone")).expect("the manifest is removed");
    daemon.reload();

    wait_for("the reload to report", Duration::from_secs(10), || {
        daemon
            .output()
            .iter()
            .any(|line| line.contains("reloaded:"))
    });

    // routing에서 빠졌다.
    assert_eq!(
        daemon.get("gone.localhost").status,
        404,
        "the route must be gone",
    );

    // process도 정지했다.
    wait_for(
        "the retired app to stop listening",
        Duration::from_secs(10),
        || !is_listening(drop_port),
    );

    // 남은 app은 그대로 서빙한다.
    assert_eq!(
        daemon.get("keep.localhost").status,
        200,
        "the remaining app must keep serving",
    );
}

/// 잘못된 manifest 집합으로 재적재하면 **아무것도 바뀌지 않는다.** (PRD §76)
///
/// daemon은 종료하지 않는다. 기동 시점의 검증 실패는 종료 코드 `78`이지만(PRD §74),
/// 재적재 실패는 이미 서빙 중인 application을 내릴 이유가 되지 않는다.
#[test]
fn a_reload_that_fails_validation_changes_nothing() {
    let apps = Apps::new();
    let port = free_port();

    echo(
        &apps,
        "blog",
        port,
        Lifecycle {
            idle: 300,
            ..Lifecycle::default()
        },
    );

    let daemon = Daemon::start(&apps);

    let before = daemon.get("blog.localhost");
    assert_eq!(before.status, 200);
    let blog_pid = before.body.clone();

    // 같은 port를 쓰는 app을 더한다. 집합 검증(§22)이 거부해야 한다.
    echo(
        &apps,
        "clash",
        port,
        Lifecycle {
            idle: 300,
            ..Lifecycle::default()
        },
    );
    daemon.reload();

    wait_for("the reload to be rejected", Duration::from_secs(10), || {
        daemon
            .output()
            .iter()
            .any(|line| line.contains("reload rejected by validation"))
    });

    // 충돌하는 app은 등록되지 않았다.
    assert_eq!(
        daemon.get("clash.localhost").status,
        404,
        "nothing may be applied from an invalid set",
    );

    // 기존 app은 같은 process로 계속 서빙한다.
    let after = daemon.get("blog.localhost");
    assert_eq!(after.status, 200);
    assert_eq!(
        after.body, blog_pid,
        "a rejected reload must not disturb the running app",
    );

    // daemon이 살아 있다는 것은 바로 위 요청이 응답한 것으로 이미 증명된다.
}
