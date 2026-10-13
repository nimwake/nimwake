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

/// 계속 실패하는 application은 **요청마다 기동을 재시도하지 않는다.** (PRD §77)
///
/// 이 테스트가 §77의 존재 이유다. backoff가 없으면 요청 8회가 spawn 8회가 된다.
/// 측정된 상한은 초당 623회였다.
///
/// 기동 시도 횟수를 daemon 로그에서 센다. 상태 코드만 보면 "시도하고 실패했다"와
/// "시도하지 않았다"를 구분할 수 없다 — 그 구분이 이 기능의 전부다.
#[test]
fn a_crash_looping_app_stops_being_respawned() {
    let apps = Apps::new();
    let port = free_port();

    // `crash-server`는 port를 bind하지 않고 바로 종료한다.
    apps.app(
        "boom",
        port,
        &[fixture("crash-server").display().to_string()],
        Lifecycle {
            idle: 60,
            startup: 3,
            ..Lifecycle::default()
        },
    );

    let daemon = Daemon::start(&apps);

    let mut codes = Vec::new();
    let mut retry_after = Vec::new();
    for _ in 0..8 {
        let response = daemon.get("boom.localhost");
        codes.push(response.status);
        retry_after.push(response.header("retry-after"));
    }

    // 처음 세 번은 시도하고 실패한다. `502`다. (PRD §24)
    assert_eq!(
        &codes[..3],
        &[502, 502, 502],
        "임계값까지는 시도해야 한다: {codes:?}",
    );
    assert!(
        retry_after[..3].iter().all(Option::is_none),
        "시도한 실패에는 Retry-After를 붙이지 않는다: {retry_after:?}",
    );

    // 그 뒤로는 시도하지 않는다. `503`과 `Retry-After`다. (PRD §77)
    assert!(
        codes[3..].iter().all(|code| *code == 503),
        "backoff는 503이다: {codes:?}",
    );
    for value in &retry_after[3..] {
        let seconds = value
            .as_deref()
            .expect("backoff에는 Retry-After가 붙는다")
            .parse::<u64>()
            .expect("Retry-After는 초 단위 정수다");
        assert!(
            (1..=30).contains(&seconds),
            "Retry-After가 범위를 벗어났다: {seconds}",
        );
    }

    // **요청 8회에 기동 시도는 3회뿐이다.**
    let attempts = daemon
        .output()
        .iter()
        .filter(|line| line.contains("starting application"))
        .count();
    assert_eq!(
        attempts, 3,
        "요청 8회가 spawn 8회가 되면 안 된다: {attempts}회 시도",
    );
}

/// 고친 manifest를 재적재하면 **backoff가 걸려 있어도 바로 기동한다.** (PRD §76, §77)
///
/// 실사용에서 가장 흔한 순서다 — 앱이 crash loop에 빠지고, 운영자가 원인을
/// 고치고, `SIGHUP`을 보낸다. 거기서 30초를 더 기다리게 만들면 고친 보람이 없다.
///
/// §76이 spec이 바뀐 application의 actor를 교체하므로 실패 이력도 함께 사라진다.
/// 그것이 의도인지 확인한다.
#[test]
fn a_reload_clears_the_backoff() {
    let apps = Apps::new();
    let broken_port = free_port();
    let good_port = free_port();

    // 깨진 앱: port를 bind하지 않고 바로 죽는다.
    apps.app(
        "fixme",
        broken_port,
        &[fixture("crash-server").display().to_string()],
        Lifecycle {
            idle: 60,
            startup: 3,
            ..Lifecycle::default()
        },
    );

    let daemon = Daemon::start(&apps);

    // backoff에 들어갈 때까지 요청한다.
    for _ in 0..4 {
        daemon.get("fixme.localhost");
    }
    let backing_off = daemon.get("fixme.localhost");
    assert_eq!(backing_off.status, 503, "backoff에 들어가야 한다");
    assert!(
        backing_off.header("retry-after").is_some(),
        "backoff에는 Retry-After가 붙는다",
    );

    // 명령을 고치고 재적재한다. port까지 바꾼다 — spec이 바뀌면 교체된다.
    apps.app(
        "fixme",
        good_port,
        &[
            fixture("echo-server").display().to_string(),
            good_port.to_string(),
        ],
        Lifecycle {
            idle: 60,
            startup: 10,
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

    // 기다리지 않고 바로 서빙한다.
    let fixed = daemon.get("fixme.localhost");
    assert_eq!(
        fixed.status, 200,
        "고친 뒤에는 backoff가 남아 있으면 안 된다",
    );
    assert!(
        fixed.header("retry-after").is_none(),
        "성공 응답에 Retry-After가 붙을 이유가 없다",
    );
}

/// 가용 메모리가 임계값 아래면 깨우지 않고 `503`으로 거절한다. (PRD §78)
///
/// 실제 기기의 가용량보다 큰 임계값을 주면 항상 부족한 상태가 된다.
/// `Retry-After`가 **없어야** 한다 — 메모리가 언제 풀릴지 모른다.
#[test]
fn a_wake_is_refused_when_the_system_is_short_on_memory() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "blog", port, Lifecycle::default());

    // 어떤 기기에서도 부족한 값. u64의 상한 근처를 쓰면 파싱 한계에 걸리므로
    // 넉넉하면서 현실적으로 도달 불가능한 값을 쓴다 (1 PiB).
    let daemon = Daemon::start_with(&apps, &["--min-free-memory-mb", "1073741824"]);

    let response = daemon.get("blog.localhost");
    assert_eq!(response.status, 503, "메모리가 부족하면 503이다");
    assert!(
        response.header("retry-after").is_none(),
        "메모리는 언제 풀릴지 모른다. Retry-After를 붙이면 거짓말이다",
    );
    assert!(
        response.body.contains("memory"),
        "본문이 이유를 말해야 한다: {}",
        response.body,
    );

    // **spawn하지 않았다.** port가 열리지 않아야 한다.
    assert!(
        !is_listening(port),
        "거절했으면 process를 띄우지 않아야 한다",
    );

    // 로그가 운영자에게 숫자를 준다.
    wait_for("the refusal to be logged", Duration::from_secs(5), || {
        daemon
            .output()
            .iter()
            .any(|line| line.contains("refusing to wake"))
    });
}

/// 임계값이 `0`(기본)이면 평소대로 깨운다. (PRD §78)
///
/// 기본값이 서비스를 막지 않는다는 것이 이 테스트의 전부다.
#[test]
fn the_default_threshold_does_not_block_anything() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "blog", port, Lifecycle::default());

    let daemon = Daemon::start_with(&apps, &["--min-free-memory-mb", "0"]);

    let response = daemon.get("blog.localhost");
    assert_eq!(response.status, 200, "기본값은 아무것도 막지 않는다");
    assert!(is_listening(port));
}

/// 임계값을 넘길 만큼 넉넉하면 깨운다. (PRD §78)
#[test]
fn a_reachable_threshold_lets_the_wake_through() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "blog", port, Lifecycle::default());

    // 1MiB. CI runner든 전화기든 이보다는 남아 있다.
    let daemon = Daemon::start_with(&apps, &["--min-free-memory-mb", "1"]);

    assert_eq!(daemon.get("blog.localhost").status, 200);
    assert!(is_listening(port));
}

/// 숫자가 아닌 값은 설정 오류(`78`)다. (PRD §74, §78)
#[test]
fn a_non_numeric_threshold_is_a_config_error() {
    let (code, output) = Daemon::run(&["--min-free-memory-mb", "lots"]);
    assert_eq!(code, 78, "설정 오류는 78이다: {output}");
    assert!(
        output.contains("--min-free-memory-mb"),
        "어느 인자가 틀렸는지 말해야 한다: {output}",
    );
}

/// 임계값이 **실제 가용량과 같은 축척**으로 비교되는지 확인한다. (PRD §78)
///
/// 이 테스트가 없으면 1024배 단위 오류가 빠져나간다. 임계값 1MiB와 1PiB는
/// `MemAvailable`을 kB로 읽든 MiB로 읽든 각각 통과·거절이므로 둘 다 통과한다.
///
/// 지금 가용량을 테스트가 직접 읽고 **그 값을 기준으로 양쪽을 고정한다.**
/// 1GiB 여유를 두어 다른 process의 변동에 흔들리지 않게 한다.
#[test]
fn the_threshold_is_compared_at_the_real_scale() {
    let available_mb = read_available_mb().expect("/proc/meminfo is readable on Linux");
    assert!(
        available_mb > 2_048,
        "이 테스트는 2GiB 이상 여유가 있어야 한다: {available_mb}MiB",
    );

    // 지금보다 1GiB 더 요구하면 거절해야 한다.
    {
        let apps = Apps::new();
        let port = free_port();
        echo(&apps, "blog", port, Lifecycle::default());
        let daemon = Daemon::start_with(
            &apps,
            &["--min-free-memory-mb", &(available_mb + 1_024).to_string()],
        );

        assert_eq!(
            daemon.get("blog.localhost").status,
            503,
            "가용량보다 1GiB 더 요구하면 거절해야 한다 (가용 {available_mb}MiB)",
        );
        assert!(!is_listening(port), "거절했으면 띄우지 않아야 한다");
    }

    // 1GiB 적게 요구하면 통과해야 한다.
    {
        let apps = Apps::new();
        let port = free_port();
        echo(&apps, "blog", port, Lifecycle::default());
        let daemon = Daemon::start_with(
            &apps,
            &["--min-free-memory-mb", &(available_mb - 1_024).to_string()],
        );

        assert_eq!(
            daemon.get("blog.localhost").status,
            200,
            "가용량보다 1GiB 적게 요구하면 통과해야 한다 (가용 {available_mb}MiB)",
        );
    }
}

/// 테스트가 직접 `MemAvailable`을 읽는다.
///
/// daemon과 같은 값을 보기 위해 **같은 방식으로** 읽는다. 다른 출처를 쓰면
/// 비교의 의미가 사라진다.
fn read_available_mb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let mut parts = rest.split_whitespace();
            let value: u64 = parts.next()?.parse().ok()?;
            if parts.next()? != "kB" {
                return None;
            }
            return Some(value / 1024);
        }
    }
    None
}

/// 파일이나 pipe로 나갈 때는 **색을 칠하지 않는다.**
///
/// 기기 72h soak의 로그에서 이것을 발견했다. `nimwaked.log` 1.19MB 중 **39.3%가
/// ANSI escape sequence**였다 — 492KB가 색상 코드다. supervisor가 stdout을
/// 파일로 리다이렉트하는데 기본값이 색 켬이기 때문이다.
///
/// 이 테스트가 있었다면 72시간을 기다리지 않고 잡혔다. harness는 pipe로 읽으므로
/// 터미널이 아니다.
#[test]
fn logs_carry_no_ansi_escapes_when_redirected() {
    let apps = Apps::new();
    let port = free_port();
    echo(&apps, "blog", port, Lifecycle::default());

    let daemon = Daemon::start(&apps);
    assert_eq!(daemon.get("blog.localhost").status, 200);

    wait_for(
        "the startup trace to be written",
        Duration::from_secs(5),
        || {
            daemon
                .output()
                .iter()
                .any(|line| line.contains("application started"))
        },
    );

    let escape = char::from(0x1b_u8);
    let output = daemon.output();
    let colored: Vec<&String> = output.iter().filter(|line| line.contains(escape)).collect();

    assert!(
        colored.is_empty(),
        "pipe로 나가는 로그에 escape sequence가 있다 ({}줄). 첫 줄: {:?}",
        colored.len(),
        colored.first(),
    );
}

/// `SIGUSR1`이 지금 상태를 로그에 쓴다. (PRD §79)
///
/// **버전이 보이는 것이 핵심이다.** `deploy`는 떠 있는 process를 바꾸지 않으므로
/// 디스크의 바이너리와 돌고 있는 것이 다를 수 있다. 실기기에서 그 차이를 모르고
/// 72시간 측정을 시작할 뻔했다 — 기동 로그를 grep해서 겨우 알아냈다.
///
/// **OS pid도 맞아야 한다.** `ProcessId`(`proc#1`)는 내부 식별자이고 `ps`와 맞춰
/// 볼 수 없다. `echo-server`가 자기 pid를 body로 돌려주므로 직접 비교한다.
#[test]
fn a_status_snapshot_reports_the_version_and_each_app() {
    let apps = Apps::new();
    let blog_port = free_port();
    let api_port = free_port();
    echo(&apps, "blog", blog_port, Lifecycle::default());
    echo(&apps, "api", api_port, Lifecycle::default());

    let daemon = Daemon::start(&apps);

    // 둘 다 잠든 상태에서 한 번.
    daemon.snapshot();
    wait_for("the first snapshot", Duration::from_secs(10), || {
        daemon
            .output()
            .iter()
            .any(|line| line.contains("status snapshot"))
    });

    let output = daemon.output();
    let header = output
        .iter()
        .find(|line| line.contains("status snapshot"))
        .expect("a snapshot header is logged");
    assert!(
        header.contains(env!("CARGO_PKG_VERSION")),
        "버전이 보여야 한다: {header}",
    );
    assert!(header.contains("apps=2"), "app 수가 보여야 한다: {header}");

    // 잠든 app은 SLEEPING이고 pid가 없다.
    let sleeping: Vec<&String> = output
        .iter()
        .filter(|line| line.contains("state=SLEEPING"))
        .collect();
    assert_eq!(sleeping.len(), 2, "둘 다 잠들어 있어야 한다: {sleeping:?}");

    // blog를 깨우고 다시 찍는다.
    let response = daemon.get("blog.localhost");
    assert_eq!(response.status, 200);
    let app_pid = response.body.trim().to_owned();
    assert!(!app_pid.is_empty(), "echo-server reports its pid");

    let before = daemon.output().len();
    daemon.snapshot();
    // 출력을 한 번만 읽는다. 두 번 읽으면 그 사이에 자라 서로 다른 것을 본다.
    wait_for("the second snapshot", Duration::from_secs(10), || {
        let output = daemon.output();
        output.len() > before
            && output[before..]
                .iter()
                .any(|line| line.contains("status snapshot"))
    });

    let output = daemon.output();
    let running = output[before..]
        .iter()
        .find(|line| line.contains("app=\"blog\"") && line.contains("state=RUNNING"))
        .expect("blog is reported as running");

    // **OS pid가 맞아야 한다.** `proc#N`은 내부 식별자라 ps와 맞출 수 없다.
    assert!(
        running.contains(&format!("pid=\"{app_pid}\"")),
        "보고된 OS pid가 app이 알린 것과 달라야 하지 않는다 (app={app_pid}): {running}",
    );
    assert!(
        running.contains("process=\"proc#"),
        "내부 식별자도 함께 보여야 한다: {running}",
    );

    // api는 여전히 잠들어 있다 — 스냅샷이 상태를 바꾸지 않는다.
    assert!(
        output[before..]
            .iter()
            .any(|line| line.contains("app=\"api\"") && line.contains("state=SLEEPING")),
        "스냅샷은 어떤 app도 깨우지 않는다",
    );
}

/// gateway가 넣은 `X-Forwarded-For`를 **application이 실제로 받는다.** (PRD §81)
///
/// 단위 테스트는 `append_forwarded_for`를 직접 부르므로 `to_backend_request`가
/// 그것을 호출하는지 모른다. 실제로 호출을 지워도 일곱 개가 모두 통과했다.
/// **받는 쪽에서 되돌려 받아야 그 빈틈이 닫힌다.**
#[test]
fn the_app_receives_the_forwarded_client_address() {
    let apps = Apps::new();
    let port = free_port();
    apps.app(
        "echo",
        port,
        &[
            fixture("header-echo-server").display().to_string(),
            port.to_string(),
        ],
        Lifecycle::default(),
    );

    let daemon = Daemon::start(&apps);
    let response = daemon.get("echo.localhost");
    assert_eq!(response.status, 200);

    // loopback으로 접속했으므로 관측된 peer는 127.0.0.1이다.
    let lower = response.body.to_lowercase();
    assert!(
        lower.contains("x-forwarded-for: 127.0.0.1"),
        "application이 X-Forwarded-For를 받지 못했다. 받은 header:\n{}",
        response.body,
    );

    // port는 들어가지 않는다 — 이 header는 주소 목록이다.
    let line = response
        .body
        .lines()
        .find(|line| line.to_lowercase().starts_with("x-forwarded-for:"))
        .expect("the header is present");
    assert!(
        !line.contains(':') || line.matches(':').count() == 1,
        "port가 들어갔다: {line}",
    );
}

/// 클라이언트가 보낸 값 **뒤에** 붙는다. 맨 오른쪽이 gateway가 관측한 것이다. (PRD §81)
///
/// gateway는 왼쪽의 값을 검증하지 않는다. 그것이 신뢰 경계이고, application이
/// 알고 읽어야 한다.
#[test]
fn a_client_supplied_chain_is_preserved_on_the_left() {
    let apps = Apps::new();
    let port = free_port();
    apps.app(
        "echo",
        port,
        &[
            fixture("header-echo-server").display().to_string(),
            port.to_string(),
        ],
        Lifecycle::default(),
    );

    let daemon = Daemon::start(&apps);

    // 클라이언트가 직접 X-Forwarded-For를 보낸다 — 스푸핑 시나리오다.
    let response = daemon
        .get_with_headers("echo.localhost", &[("x-forwarded-for", "198.51.100.23")])
        .expect("gateway responds");
    assert_eq!(response.status, 200);

    let line = response
        .body
        .lines()
        .find(|line| line.to_lowercase().starts_with("x-forwarded-for:"))
        .unwrap_or_else(|| panic!("헤더가 없다:\n{}", response.body));

    assert!(
        line.contains("198.51.100.23"),
        "클라이언트가 보낸 값이 남아야 한다: {line}",
    );
    assert!(
        line.trim_end().ends_with("127.0.0.1"),
        "맨 오른쪽이 gateway가 관측한 것이어야 한다: {line}",
    );
}

/// `--version`은 버전 한 줄만 내고 `0`으로 끝낸다. (PRD §74)
///
/// 호스트가 **daemon을 띄우지 않고** 이 binary가 무엇인지 물을 수 있어야 한다.
/// 모르는 인자는 설정 오류(`78`)이므로, 새 flag를 붙일지 결정하려면 먼저 버전을
/// 알아야 한다 — `nimwake-android`가 `--min-free-memory-mb`에서 그 문제를
/// 만났다 (nimwake-android#43).
#[test]
fn the_version_flag_prints_one_line_and_exits_zero() {
    for flag in ["--version", "-V"] {
        let (code, output) = Daemon::run(&[flag]);
        assert_eq!(code, 0, "`{flag}`는 0으로 끝난다: {output}");

        let lines: Vec<&str> = output.lines().filter(|line| !line.is_empty()).collect();
        assert_eq!(lines.len(), 1, "`{flag}`는 한 줄만 낸다: {output:?}");
        assert!(
            lines[0].starts_with("nimwaked "),
            "`{flag}`의 출력 형식이 다르다: {}",
            lines[0],
        );
        assert!(
            lines[0].contains(env!("CARGO_PKG_VERSION")),
            "`{flag}`가 crate 버전을 내지 않았다: {}",
            lines[0],
        );
    }
}

/// `--version`은 manifest를 읽지 않는다. (PRD §74)
///
/// 디렉터리가 없어도 버전은 답해야 한다 — 호스트가 배포 **전에** 물을 수 있다.
#[test]
fn the_version_flag_does_not_need_manifests() {
    let (code, output) = Daemon::run(&["--apps", "/nonexistent", "--version"]);
    assert_eq!(code, 0, "manifest 없이도 버전은 답한다: {output}");
    assert!(output.contains("nimwaked "), "{output}");
}
