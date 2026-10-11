//! Gateway를 통과하는 end-to-end 사이클.
//!
//! 요구사항: PRD §23–§26, §27–§29, §31, §69.
//! 마일스톤: `.docs/phases/phase-1/03-milestones/03-v0.0.3-http-scale-to-zero.md`
//!
//! 이 파일이 증명하는 것은 Nimwake의 핵심 가설이다.
//!
//! ```text
//! request → wake → serve → idle → sleep
//! ```
//!
//! fixture는 자기 pid를 body로 돌려준다. "같은 PID인가 / 새 PID인가"를 HTTP로 직접 본다.
//!
//! HTTP client는 blocking이므로 항상 `spawn_blocking`으로 부른다.
//! 테스트 thread에서 곧바로 부르면 같은 runtime 위에 있는 gateway task가 굶는다.

#![cfg(unix)]

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{
    bin, cmd, free_port, get, is_listening, request, spec, wait_for, with_pending_limit,
    with_response_header_timeout, with_timeouts, HttpResponse,
};
use nimwake_core::app::AppSpec;
use nimwake_core::runtime::ProcessRuntime;
use nimwake_core::Limits;
use nimwake_core::{AppRegistry, AppStateName, Gateway, DEFAULT_GATEWAY_PORT};

/// 띄운 gateway와, 그것을 정리하는 방법.
struct Harness {
    port: u16,
    registry: Arc<AppRegistry>,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

impl Harness {
    /// gateway를 `port 0`에 bind하고 실제 배정된 값을 읽는다.
    /// 고정 port를 쓰면 테스트끼리 충돌한다.
    async fn start(specs: Vec<AppSpec>) -> Self {
        let registry = AppRegistry::build(
            specs,
            DEFAULT_GATEWAY_PORT,
            Arc::new(ProcessRuntime::new()),
            Limits::default(),
        )
        .expect("specs validate");
        let registry = Arc::new(registry);

        let gateway = Gateway::bind(SocketAddr::from(([127, 0, 0, 1], 0)), Arc::clone(&registry))
            .await
            .expect("gateway binds");
        let port = gateway.local_addr().expect("gateway has an address").port();

        let (shutdown, shutdown_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(gateway.serve(async move {
            let _ = shutdown_rx.await;
        }));

        Self {
            port,
            registry,
            shutdown,
        }
    }

    async fn get(&self, host: &str) -> HttpResponse {
        let (port, host) = (self.port, host.to_owned());
        tokio::task::spawn_blocking(move || get(port, &host))
            .await
            .expect("client task finished")
            .expect("gateway responds")
    }

    async fn post(&self, host: &str, path: &str, body: &str) -> HttpResponse {
        let (port, host, path, body) =
            (self.port, host.to_owned(), path.to_owned(), body.to_owned());
        tokio::task::spawn_blocking(move || request(port, "POST", &path, &host, Some(&body)))
            .await
            .expect("client task finished")
            .expect("gateway responds")
    }

    async fn state(&self, name: &str) -> AppStateName {
        self.registry
            .get(name)
            .expect("app is registered")
            .status()
            .await
            .expect("actor is alive")
            .state
    }

    async fn wait_for_state(&self, name: &str, target: AppStateName, deadline: Duration) {
        let started = Instant::now();
        while started.elapsed() < deadline {
            if self.state(name).await == target {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "`{name}` never reached {target}, still {}",
            self.state(name).await
        );
    }

    async fn stop(self) {
        let _ = self.shutdown.send(());
        self.registry.shutdown().await;
    }
}

fn echo_app(name: &str, port: u16, idle_seconds: u64) -> AppSpec {
    with_timeouts(
        spec(name, port, cmd(bin::ECHO, &[port.to_string()])),
        5,
        2,
        idle_seconds,
    )
}

/// 마일스톤 Acceptance 전체.
///
/// ```text
/// curl → process starts → 200
/// curl again → same PID
/// wait idle timeout → process disappears
/// curl again → new PID
/// ```
#[tokio::test(flavor = "multi_thread")]
async fn request_wakes_the_app_serves_it_sleeps_and_wakes_again() {
    let app_port = free_port();
    let harness = Harness::start(vec![echo_app("hello", app_port, 1)]).await;

    assert!(!is_listening(app_port), "the app starts asleep");

    let first = harness.get("hello.localhost").await;
    assert_eq!(first.status, 200);
    assert!(is_listening(app_port), "the app is awake now");

    let second = harness.get("hello.localhost").await;
    assert_eq!(second.status, 200);
    assert_eq!(
        first.pid(),
        second.pid(),
        "a warm request must reuse the running process",
    );

    // port가 닫히기를 기다린 뒤 state를 단언하면 안 된다.
    // port는 STOPPING 중에 닫히고, SLEEPING은 그 완료 event가 mailbox를 한 바퀴 돈 뒤다.
    // SLEEPING은 process가 사라졌음을 함의하지만 그 역은 아니므로 state를 기다린다.
    harness
        .wait_for_state("hello", AppStateName::Sleeping, Duration::from_secs(8))
        .await;
    assert!(!is_listening(app_port), "SLEEPING이면 process가 없다");

    let third = harness.get("hello.localhost").await;
    assert_eq!(third.status, 200);
    assert_ne!(
        first.pid(),
        third.pid(),
        "waking up again must start a new process",
    );

    harness.stop().await;
}

/// 요청이 계속 오는 동안에는 idle timer가 매번 리셋된다. (PRD §25, §29)
#[tokio::test(flavor = "multi_thread")]
async fn a_steady_stream_of_requests_keeps_the_app_awake() {
    let app_port = free_port();
    let harness = Harness::start(vec![echo_app("busy", app_port, 1)]).await;

    let first = harness.get("busy.localhost").await;
    assert_eq!(first.status, 200);

    // idle timeout이 지나는 동안 계속 요청을 보낸다. 매 요청이 timer를 리셋해야 한다.
    for _ in 0..6 {
        let response = harness.get("busy.localhost").await;
        assert_eq!(response.status, 200);
        assert_eq!(
            response.pid(),
            first.pid(),
            "the app must not have restarted while requests kept arriving",
        );
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    harness.stop().await;
}

/// Host header가 routing의 유일한 근거다. (PRD §23)
#[tokio::test(flavor = "multi_thread")]
async fn routing_is_driven_by_the_host_header() {
    let ports = [free_port(), free_port()];
    let harness = Harness::start(vec![
        echo_app("one", ports[0], 60),
        echo_app("two", ports[1], 60),
    ])
    .await;

    let one = harness.get("one.localhost").await;
    let two = harness.get("two.localhost").await;
    assert_eq!(one.status, 200);
    assert_eq!(two.status, 200);
    assert_ne!(
        one.pid(),
        two.pid(),
        "each host must reach its own application",
    );

    // port suffix와 대소문자는 무시한다
    let normalized = harness.get("ONE.localhost:8080").await;
    assert_eq!(normalized.status, 200);
    assert_eq!(normalized.pid(), one.pid());

    harness.stop().await;
}

/// 등록되지 않은 host는 `404`이고, 아무것도 깨우지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_host_is_404_and_wakes_nothing() {
    let app_port = free_port();
    let harness = Harness::start(vec![echo_app("hello", app_port, 60)]).await;

    let response = harness.get("nope.localhost").await;
    assert_eq!(response.status, 404);
    assert!(!is_listening(app_port), "404 must not start anything");
    assert_eq!(harness.state("hello").await, AppStateName::Sleeping);

    harness.stop().await;
}

/// cold start를 거쳐도 POST body가 그대로 전달되어야 한다. (PRD §24)
///
/// header 수신 → acquire(필요하면 cold start) → backend ready → body 전달.
#[tokio::test(flavor = "multi_thread")]
async fn a_post_body_survives_a_cold_start() {
    let app_port = free_port();
    let harness = Harness::start(vec![echo_app("echo", app_port, 60)]).await;

    assert!(
        !is_listening(app_port),
        "the body must cross a real cold start"
    );

    let payload = "nimwake body round-trip";
    let response = harness.post("echo.localhost", "/echo", payload).await;

    assert_eq!(response.status, 200);
    assert_eq!(
        response.body, payload,
        "the body must reach the backend intact",
    );

    harness.stop().await;
}

/// STOPPING 중 도착한 요청은 실패시키지 않고 waiter로 등록한다. (PRD §31)
///
/// `stubborn-server`는 SIGTERM을 무시하므로 STOPPING이 shutdown timeout만큼 이어진다.
/// 그 창 안에 요청을 넣으면 stop 완료 후 재기동으로 처리되어야 한다.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_during_stopping_waits_for_the_restart() {
    let app_port = free_port();
    let harness = Harness::start(vec![with_timeouts(
        spec(
            "stubborn",
            app_port,
            cmd(bin::STUBBORN, &[app_port.to_string()]),
        ),
        10,
        2,
        1,
    )])
    .await;

    let first = harness.get("stubborn.localhost").await;
    assert_eq!(first.status, 200);

    harness
        .wait_for_state("stubborn", AppStateName::Stopping, Duration::from_secs(5))
        .await;

    // STOPPING 중에 도착한 요청. 실패하지 않고 기다렸다가 새 process로 응답해야 한다.
    let second = harness.get("stubborn.localhost").await;
    assert_eq!(
        second.status, 200,
        "a request during STOPPING must not fail"
    );
    assert_ne!(
        first.pid(),
        second.pid(),
        "shutdown is not cancelled, so the request is served by a new process",
    );

    harness.stop().await;
}

/// pending waiter 상한을 넘으면 `503`이다. 그리고 process는 하나만 뜬다. (PRD §69, §40)
#[tokio::test(flavor = "multi_thread")]
async fn pending_waiters_beyond_the_limit_get_503() {
    const LIMIT: usize = 2;

    let app_port = free_port();
    let harness = Harness::start(vec![with_pending_limit(
        with_timeouts(
            spec(
                "slow",
                app_port,
                // cold start가 충분히 느려야 모든 요청이 STARTING 구간에 겹친다.
                cmd(bin::SLOW_START, &[app_port.to_string(), "1500".to_owned()]),
            ),
            10,
            2,
            60,
        ),
        LIMIT,
    )])
    .await;

    // 요청은 전부 blocking client이므로 spawn_blocking으로 동시에 띄운다.
    // harness를 빌려오면 'static이 아니라서 spawn할 수 없다. port만 복사해 넘긴다.
    let gateway = harness.port;
    let clients: Vec<_> = (0..10)
        .map(|_| tokio::task::spawn_blocking(move || get(gateway, "slow.localhost")))
        .collect();

    let mut responses = Vec::with_capacity(clients.len());
    for client in clients {
        responses.push(
            client
                .await
                .expect("client task finished")
                .expect("gateway responds"),
        );
    }

    let ok: Vec<_> = responses.iter().filter(|r| r.status == 200).collect();
    let rejected = responses.iter().filter(|r| r.status == 503).count();

    assert!(!ok.is_empty(), "waiters within the limit must be served");
    assert!(
        rejected > 0,
        "requests beyond the limit must be rejected with 503, got {:?}",
        responses.iter().map(|r| r.status).collect::<Vec<_>>(),
    );
    assert!(
        ok.len() <= LIMIT,
        "at most {LIMIT} waiters may ride the cold start, got {}",
        ok.len(),
    );

    // 거절된 요청이 process를 추가로 만들지 않았어야 한다.
    let pids: std::collections::HashSet<_> = ok.iter().map(|r| r.pid()).collect();
    assert_eq!(pids.len(), 1, "overload must not start extra processes");

    harness.stop().await;
}

/// 응답 body가 끝날 때까지 lease가 살아 있어야 한다. (PRD §25–§26)
///
/// `slow-body-server`는 header를 먼저 보내고 body를 2초에 걸쳐 흘린다.
/// idle timeout은 1초다. service 함수가 반환하는 시점에 lease를 반납한다면
/// 응답 도중에 idle shutdown이 걸려 body가 잘린다.
#[tokio::test(flavor = "multi_thread")]
async fn a_lease_outlives_the_response_body() {
    let app_port = free_port();
    let harness = Harness::start(vec![with_timeouts(
        spec(
            "stream",
            app_port,
            cmd(bin::SLOW_BODY, &[app_port.to_string()]),
        ),
        5,
        2,
        1,
    )])
    .await;

    let response = harness.get("stream.localhost").await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body.len(),
        200,
        "the response body was truncated: {:?}",
        response.body,
    );
    assert_eq!(harness.state("stream").await, AppStateName::Running);

    // 응답이 끝났으니 이제는 잠들어야 한다.
    wait_for(
        "the app to sleep once the response finished",
        Duration::from_secs(8),
        || !is_listening(app_port),
    )
    .await;

    harness.stop().await;
}

/// proxy가 실패해도 lease는 회수된다. (PRD §26)
///
/// `deaf-server`는 bind만 하고 응답하지 않는다. readiness는 통과하므로 RUNNING이 되고,
/// 모든 요청은 502가 된다. 그 뒤에도 idle timeout이 동작한다면 `active_requests`가
/// 0으로 내려갔다는 뜻이다. lease가 새면 app은 영원히 잠들지 못한다.
#[tokio::test(flavor = "multi_thread")]
async fn a_proxy_failure_returns_502_and_still_releases_the_lease() {
    let app_port = free_port();
    let harness = Harness::start(vec![with_timeouts(
        spec("deaf", app_port, cmd(bin::DEAF, &[app_port.to_string()])),
        5,
        2,
        1,
    )])
    .await;

    for _ in 0..3 {
        let response = harness.get("deaf.localhost").await;
        assert_eq!(response.status, 502, "an unusable backend must produce 502");
    }

    wait_for(
        "the app to sleep after failed proxies",
        Duration::from_secs(8),
        || !is_listening(app_port),
    )
    .await;

    harness.stop().await;
}

/// 기동에 실패하는 application은 `502`다. overload(`503`)와 구분한다.
#[tokio::test(flavor = "multi_thread")]
async fn an_app_that_cannot_start_returns_502() {
    let app_port = free_port();
    let harness = Harness::start(vec![with_timeouts(
        spec("crash", app_port, cmd(bin::CRASH, &[])),
        5,
        2,
        60,
    )])
    .await;

    let response = harness.get("crash.localhost").await;
    assert_eq!(response.status, 502);
    assert_eq!(harness.state("crash").await, AppStateName::Failed);

    harness.stop().await;
}

/// Host header가 없으면 routing할 근거가 없다.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_without_a_host_header_is_rejected() {
    let app_port = free_port();
    let harness = Harness::start(vec![echo_app("hello", app_port, 60)]).await;
    let gateway = harness.port;

    let raw = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", gateway))?;
        stream.write_all(b"GET / HTTP/1.0\r\nconnection: close\r\n\r\n")?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw)?;
        Ok::<_, std::io::Error>(raw)
    })
    .await
    .expect("client task finished")
    .expect("gateway responds");

    assert!(raw.contains(" 400 "), "unexpected response: {raw:?}");
    assert!(
        !is_listening(app_port),
        "a rejected request must not wake anything",
    );

    harness.stop().await;
}

/// 연결을 받아 두고 응답하지 않는 backend는 `504`로 끝낸다. (PRD §75)
///
/// `mute-server`는 `deaf-server`와 다르다. deaf는 accept하고 끊으므로 proxy가
/// 실패해 `502`가 된다. 이쪽은 연결을 **붙잡는다.** 요청을 읽지도, 응답을 쓰지도,
/// 끊지도 않는다.
///
/// port는 열려 있으므로 readiness probe도 startup timeout도 이것을 걸러내지
/// 못한다. startup timeout은 readiness까지만 덮는다. 그래서 요청 쪽에 상한이 필요하다.
///
/// 상한이 없던 동안 이 요청은 client가 끊을 때까지 끝나지 않았다.
#[tokio::test(flavor = "multi_thread")]
async fn a_backend_that_never_answers_is_cut_off_with_504() {
    let app_port = free_port();
    let harness = Harness::start(vec![with_response_header_timeout(
        with_timeouts(
            spec("mute", app_port, cmd(bin::MUTE, &[app_port.to_string()])),
            5,
            60,
            1,
        ),
        1,
    )])
    .await;

    let started = Instant::now();
    let response = harness.get("mute.localhost").await;
    let elapsed = started.elapsed();

    assert_eq!(
        response.status, 504,
        "a backend that never answers must produce 504, body: {:?}",
        response.body,
    );

    // 상한이 1초다. 기본값 60초를 기다렸다면 상한이 적용되지 않은 것이다.
    assert!(
        elapsed < Duration::from_secs(10),
        "the request should end at the limit, took {elapsed:?}",
    );

    // application은 살아 있다. 끊은 것은 요청 하나다.
    assert_eq!(harness.state("mute").await, AppStateName::Running);

    harness.stop().await;
}

/// 상한은 응답 header까지만 적용한다. body 전송에는 적용하지 않는다. (PRD §75)
///
/// **이 테스트가 계약의 핵심이다.** 전체 요청에 상한을 두면 큰 파일이나 천천히
/// 흐르는 응답이 끊긴다. 막으려는 것은 느린 응답이 아니라 오지 않는 응답이다.
///
/// `slow-body-server`는 header를 즉시 보내고 body를 2초(20 x 100ms)에 걸쳐 흘린다.
/// 상한을 1초로 두었으므로, body까지 상한에 들어간다면 응답이 잘린다.
#[tokio::test(flavor = "multi_thread")]
async fn a_slow_response_body_is_not_cut_off_by_the_header_timeout() {
    let app_port = free_port();
    let harness = Harness::start(vec![with_response_header_timeout(
        with_timeouts(
            spec(
                "stream",
                app_port,
                cmd(bin::SLOW_BODY, &[app_port.to_string()]),
            ),
            5,
            2,
            60,
        ),
        1,
    )])
    .await;

    let response = harness.get("stream.localhost").await;

    assert_eq!(response.status, 200, "body: {}", response.body);
    assert_eq!(
        response.body.len(),
        200,
        "the body must survive a limit shorter than the body takes: {:?}",
        response.body,
    );

    harness.stop().await;
}

/// port가 이미 다른 process의 것이면 application을 시작하지 않는다.
///
/// readiness probe는 "port에 붙을 수 있나"만 묻는다. 그래서 사전 검사가 없으면
/// Nimwake가 띄우지 않은 listener를 자기 application으로 착각하고, 트래픽이
/// 그쪽으로 간다. 자기 자식은 bind에 실패해 죽는데 daemon은 그것을 모른다.
///
/// daemon을 SIGKILL하면 application이 남으므로(destructor가 돌지 않는다) 실제로
/// 일어나는 상황이다. Nimwake는 process를 인수하지 않는다. (PRD §71)
///
/// `echo-server`는 자기 pid를 body로 돌려준다. 그래서 "누가 응답했는가"를
/// HTTP로 직접 구별할 수 있다.
#[tokio::test(flavor = "multi_thread")]
async fn a_port_held_by_a_foreign_process_is_not_adopted() {
    let app_port = free_port();

    // Nimwake와 무관한 listener. 이 테스트가 소유하고 정리한다.
    let mut foreign = std::process::Command::new(bin::ECHO)
        .arg(app_port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the foreign listener starts");

    wait_for(
        "the foreign listener to bind",
        Duration::from_secs(5),
        || is_listening(app_port),
    )
    .await;

    let foreign_pid = foreign.id().to_string();

    let harness = Harness::start(vec![with_timeouts(
        spec("victim", app_port, cmd(bin::ECHO, &[app_port.to_string()])),
        5,
        60,
        1,
    )])
    .await;

    let response = harness.get("victim.localhost").await;

    // 고치기 전에는 200에 외부 process의 pid가 담겨 돌아왔다.
    assert_eq!(
        response.status, 502,
        "an occupied port must fail the start, got {} with body {:?}",
        response.status, response.body,
    );
    assert!(
        !response.body.contains(&foreign_pid),
        "the foreign process ({foreign_pid}) must never serve through Nimwake",
    );

    // app은 RUNNING이 아니다. 시작한 적이 없다.
    assert_ne!(harness.state("victim").await, AppStateName::Running);

    // 외부 listener는 Nimwake가 건드리지 않는다. 남의 것이다.
    assert!(
        is_listening(app_port),
        "Nimwake must not stop a process it does not own",
    );

    let _ = foreign.kill();
    let _ = foreign.wait();
}

/// §24가 보장 범위로 적은 메서드 여섯이 모두 통과한다.
///
/// 지금까지 테스트는 GET과 POST만 썼다. 나머지 넷은 동작하지만 아무것도
/// 고정하고 있지 않았다. method 필터가 들어오거나 `to_backend_request`가 무언가를
/// 떨어뜨리면 조용히 깨진다.
///
/// HEAD는 의미가 다르다 — 응답에 body가 없다. 그래서 따로 단언한다.
///
/// (조사하다 한 번 헛짚었다. `curl -X HEAD`는 응답이 bodiless인 것을 모르고
/// 기다리므로 멈춘 것처럼 보인다. `curl -I`로 보면 정상이다. gateway 문제가
/// 아니었다.)
#[tokio::test(flavor = "multi_thread")]
async fn every_contract_method_passes_through() {
    let app_port = free_port();
    let harness = Harness::start(vec![with_timeouts(
        spec("verbs", app_port, cmd(bin::ECHO, &[app_port.to_string()])),
        5,
        60,
        1,
    )])
    .await;

    let port = harness.port;
    for method in ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH"] {
        let response = tokio::task::spawn_blocking(move || {
            request(port, method, "/", "verbs.localhost", None)
        })
        .await
        .expect("client task finished")
        .unwrap_or_else(|error| panic!("`{method}` failed: {error}"));

        assert_eq!(response.status, 200, "`{method}` must reach the backend");

        if method == "HEAD" {
            assert!(
                response.body.is_empty(),
                "HEAD must not carry a body, got {:?}",
                response.body,
            );
        } else {
            assert!(
                !response.body.is_empty(),
                "`{method}` must carry the backend's response",
            );
        }
    }

    harness.stop().await;
}
