//! 공개 API로 본 lifecycle 전체 경로.
//!
//! 요구사항: PRD §10 Application Lifecycle, §27 Scale-to-Zero, §32–§33.
//!
//! 가상 시계(`start_paused`)를 사용해 idle timeout을 결정적으로 재현한다.

mod common;

use std::sync::Arc;
use std::time::Duration;

use nimwake_core::app;
use nimwake_core::error::AcquireError;
use nimwake_core::eviction;
use nimwake_core::registry::{self, AppRegistry, DEFAULT_GATEWAY_PORT};
use nimwake_core::runtime::FakeRuntime;
use nimwake_core::Limits;
use nimwake_core::{AppStateName, Runtime};

fn spawn_app(runtime: &Arc<FakeRuntime>, idle_seconds: u64) -> nimwake_core::AppHandle {
    app::spawn(
        Arc::new(common::spec(idle_seconds, 64)),
        Arc::clone(runtime) as Arc<dyn Runtime>,
    )
}

#[tokio::test(start_paused = true)]
async fn request_wakes_the_app_and_idle_puts_it_back_to_sleep() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 1);

    // SLEEPING -> STARTING -> RUNNING
    let lease = handle.acquire().await.expect("cold start succeeds");
    let first = handle.status().await.expect("manager is alive");
    assert_eq!(first.state, AppStateName::Running);
    assert_eq!(first.active_requests, 1);
    assert_eq!(runtime.alive_processes(), 1);

    // 요청이 끝나면 idle timer가 걸린다.
    drop(lease);
    common::wait_for_active_requests(&handle, 0).await;

    // RUNNING -> STOPPING -> SLEEPING
    tokio::time::advance(Duration::from_secs(2)).await;
    common::wait_for_state(&handle, AppStateName::Sleeping).await;
    assert_eq!(runtime.alive_processes(), 0, "process must be gone");

    // 다시 요청하면 새 process로 깨어난다.
    let lease = handle.acquire().await.expect("re-wake succeeds");
    let second = handle.status().await.expect("manager is alive");
    assert_eq!(second.state, AppStateName::Running);
    assert_eq!(runtime.start_count(), 2);
    assert_ne!(first.process_id, second.process_id, "새 process여야 한다");

    drop(lease);
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn active_lease_prevents_idle_shutdown() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 1);

    let lease = handle.acquire().await.expect("cold start succeeds");

    // lease를 쥔 채 idle timeout을 한참 넘겨도 종료되지 않는다. (Invariant 5)
    tokio::time::advance(Duration::from_secs(30)).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let status = handle.status().await.expect("manager is alive");
    assert_eq!(status.state, AppStateName::Running);
    assert_eq!(status.active_requests, 1);
    assert_eq!(runtime.stop_count(), 0);

    drop(lease);
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn process_crash_fails_the_app_and_the_next_request_restarts_it() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);

    let lease = handle.acquire().await.expect("cold start succeeds");
    let process = handle
        .status()
        .await
        .expect("manager is alive")
        .process_id
        .expect("running app has a process");

    runtime.crash(process);
    drop(lease);

    common::wait_for_state(&handle, AppStateName::Failed).await;
    let failed = handle.status().await.expect("manager is alive");
    assert_eq!(failed.active_requests, 0, "죽은 process의 lease는 무효다");
    assert!(failed.last_error.is_some());

    // FAILED는 terminal이 아니다. 다음 요청이 다시 기동한다. (PRD §33)
    let lease = handle.acquire().await.expect("retry succeeds");
    let status = handle.status().await.expect("manager is alive");
    assert_eq!(status.state, AppStateName::Running);
    assert_eq!(runtime.start_count(), 2);
    assert_ne!(status.process_id, Some(process));

    drop(lease);
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn startup_failure_is_reported_and_retryable() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);

    runtime.fail_next_starts(1);
    let error = handle.acquire().await.expect_err("startup fails");
    assert!(matches!(error, nimwake_core::AcquireError::StartFailed(_)));

    let status = handle.status().await.expect("manager is alive");
    assert_eq!(status.state, AppStateName::Failed);
    assert_eq!(runtime.alive_processes(), 0);

    let lease = handle.acquire().await.expect("retry succeeds");
    assert_eq!(
        handle.status().await.expect("manager is alive").state,
        AppStateName::Running
    );

    drop(lease);
    handle.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn registry_routes_by_host_and_shutdown_stops_every_process() {
    let runtime = Arc::new(FakeRuntime::default());

    let mut blog = common::spec(60, 64);
    blog.name = "blog".to_owned();
    blog.server.host = "blog.localhost".to_owned();
    blog.server.port = 3101;

    let mut api = common::spec(60, 64);
    api.name = "api".to_owned();
    api.server.host = "api.localhost".to_owned();
    api.server.port = 3102;

    let registry = AppRegistry::build(
        vec![blog, api],
        DEFAULT_GATEWAY_PORT,
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        Limits::default(),
    )
    .expect("registry builds");

    assert_eq!(registry.len(), 2);
    assert!(registry.resolve("nope.localhost").is_none());

    // Host header의 port suffix와 대소문자는 무시한다. (PRD §23)
    let blog = registry
        .resolve("Blog.localhost:8080")
        .expect("host resolves");
    let lease = blog.acquire().await.expect("cold start succeeds");
    assert_eq!(lease.backend().addr().port(), 3101);
    assert_eq!(runtime.alive_processes(), 1);

    drop(lease);

    // daemon이 내려가면 child도 함께 정리된다. (Invariant 10 / PRD §71)
    registry.shutdown().await;
    common::wait_for_zero_alive(&runtime).await;
}

#[test]
fn registry_rejects_conflicting_manifests() {
    let mut first = common::spec(60, 64);
    first.name = "blog".to_owned();
    first.server.host = "blog.localhost".to_owned();
    first.server.port = 3101;

    let mut second = common::spec(60, 64);
    second.name = "api".to_owned();
    second.server.host = "api.localhost".to_owned();
    second.server.port = 3101;

    let error = registry::validate(&[first, second], DEFAULT_GATEWAY_PORT)
        .expect_err("port conflict is rejected at load time");

    assert!(error.to_string().contains("duplicate port 3101"));
}

// 재적재. (PRD §76)

/// **manifest에 대문자나 port로 적힌 alias도 찾아진다.** (PRD §87)
///
/// 색인에 넣을 때 정규화해야 한다. `resolve`가 들어오는 `Host`만 정규화하고
/// 색인은 날것으로 두면, manifest를 `WWW.Example.COM`으로 적은 사람의 alias가
/// 영영 맞지 않는다 — **뮤테이션으로 이 구멍을 찾았다.**
#[tokio::test]
async fn an_alias_written_oddly_is_still_found() {
    let runtime = Arc::new(FakeRuntime::default());
    let mut blog = app_spec("blog", 3101);
    blog.server.aliases = vec![
        "WWW.Blog.LocalHost:9999".to_owned(),
        "  tunnel.localhost  ".to_owned(),
    ];

    let registry = registry_of(&runtime, vec![blog]);

    for host in ["www.blog.localhost", "tunnel.localhost"] {
        assert!(
            registry.resolve(host).is_some(),
            "`{host}`가 찾아져야 한다 — 색인을 정규화하지 않으면 못 찾는다",
        );
    }

    // 들어오는 쪽도 정규화되므로 어느 형태로 와도 같다.
    assert!(registry.resolve("WWW.BLOG.localhost:1234").is_some());

    registry.shutdown().await;
}

/// 이름·host·port를 바꾼 spec을 만든다.
fn app_spec(name: &str, port: u16) -> nimwake_core::app::AppSpec {
    let mut spec = common::spec(60, 64);
    spec.name = name.to_owned();
    spec.server.host = format!("{name}.localhost");
    spec.server.port = port;
    spec
}

fn registry_of(runtime: &Arc<FakeRuntime>, specs: Vec<nimwake_core::app::AppSpec>) -> AppRegistry {
    AppRegistry::build(
        specs,
        DEFAULT_GATEWAY_PORT,
        Arc::clone(runtime) as Arc<dyn Runtime>,
        Limits::default(),
    )
    .expect("registry builds")
}

/// **이 테스트가 §76의 전부다.** application 하나를 더해도 나머지는 건드리지 않는다.
///
/// 그것이 안 되면 daemon 재시작과 다를 바 없고, 이 기능의 존재 이유가 사라진다.
/// "건드리지 않았다"는 것을 상태로 본다 — 돌고 있던 app이 RUNNING으로 남아야 한다.
#[tokio::test(start_paused = true)]
async fn adding_an_app_leaves_the_running_ones_alone() {
    let runtime = Arc::new(FakeRuntime::default());
    let registry = registry_of(&runtime, vec![app_spec("blog", 3101)]);

    // blog를 깨워 둔다. 재적재가 이것을 건드리면 안 된다.
    let blog = registry.get("blog").expect("blog is registered");
    let lease = blog.acquire().await.expect("cold start succeeds");
    assert_eq!(
        blog.status().await.expect("manager is alive").state,
        AppStateName::Running
    );
    let process_before = blog.status().await.expect("manager is alive").process_id;

    let outcome = registry
        .reload(vec![app_spec("blog", 3101), app_spec("api", 3102)])
        .await
        .expect("the new set validates");

    assert_eq!(outcome.added, ["api"]);
    assert_eq!(outcome.unchanged, ["blog"]);
    assert!(outcome.removed.is_empty());
    assert!(outcome.replaced.is_empty());

    // blog는 같은 process로 계속 RUNNING이다.
    let after = blog.status().await.expect("manager is alive");
    assert_eq!(after.state, AppStateName::Running, "blog must keep running");
    assert_eq!(
        after.process_id, process_before,
        "the process must not change"
    );
    assert_eq!(after.active_requests, 1, "the live lease must survive");

    // 새 app은 잠든 상태로 등록된다.
    let api = registry.get("api").expect("api is registered");
    assert_eq!(
        api.status().await.expect("manager is alive").state,
        AppStateName::Sleeping
    );
    assert_eq!(registry.len(), 2);
    assert!(registry.resolve("api.localhost").is_some());

    drop(lease);
    registry.shutdown().await;
}

/// manifest에서 사라진 application은 정지되고 routing에서 빠진다. (PRD §76)
#[tokio::test(start_paused = true)]
async fn removing_an_app_stops_it_and_drops_the_route() {
    let runtime = Arc::new(FakeRuntime::default());
    let registry = registry_of(
        &runtime,
        vec![app_spec("blog", 3101), app_spec("api", 3102)],
    );

    let api = registry.get("api").expect("api is registered");
    let lease = api.acquire().await.expect("cold start succeeds");
    drop(lease);

    let outcome = registry
        .reload(vec![app_spec("blog", 3101)])
        .await
        .expect("the new set validates");

    assert_eq!(outcome.removed, ["api"]);
    assert_eq!(outcome.unchanged, ["blog"]);

    assert_eq!(registry.len(), 1);
    assert!(
        registry.resolve("api.localhost").is_none(),
        "the route must be gone",
    );
    assert!(registry.get("api").is_none());

    // actor가 내려갔으므로 더는 응답하지 않는다.
    assert!(
        api.status().await.is_err(),
        "the retired actor must be gone",
    );

    registry.shutdown().await;
}

/// spec이 바뀐 application은 정지된다. 다음 요청이 새 spec으로 기동한다. (PRD §76)
///
/// `[lifecycle]` timeout만 바뀐 경우도 정지시킨다. 무엇이 바뀌었는지에 따라
/// 다르게 처리하면 규칙이 늘어나고, 정지는 scale-to-zero에서 정상 경로다.
#[tokio::test(start_paused = true)]
async fn changing_a_spec_replaces_the_app() {
    let runtime = Arc::new(FakeRuntime::default());
    let registry = registry_of(&runtime, vec![app_spec("blog", 3101)]);

    let before = registry.get("blog").expect("blog is registered");
    let lease = before.acquire().await.expect("cold start succeeds");
    drop(lease);

    // idle timeout만 바꾼다. 그래도 교체 대상이다.
    let mut changed = app_spec("blog", 3101);
    changed.lifecycle.idle_timeout_seconds = 120;

    let outcome = registry
        .reload(vec![changed])
        .await
        .expect("the new set validates");

    assert_eq!(outcome.replaced, ["blog"]);
    assert!(outcome.unchanged.is_empty());
    assert!(outcome.added.is_empty());

    // 이전 actor는 내려갔고, 같은 이름으로 새 actor가 서 있다.
    assert!(before.status().await.is_err(), "the old actor must be gone");

    let after = registry.get("blog").expect("blog is registered again");
    let status = after.status().await.expect("the new actor is alive");
    assert_eq!(status.state, AppStateName::Sleeping);
    assert_eq!(after.spec().lifecycle.idle_timeout_seconds, 120);

    registry.shutdown().await;
}

/// 검증이 실패하면 **아무것도 바꾸지 않는다.** (PRD §76)
///
/// §22의 검증은 집합 전체를 보므로 일부만 적용하면 어떤 manifest 파일과도
/// 일치하지 않는 상태가 남는다.
#[tokio::test(start_paused = true)]
async fn an_invalid_set_changes_nothing() {
    let runtime = Arc::new(FakeRuntime::default());
    let registry = registry_of(
        &runtime,
        vec![app_spec("blog", 3101), app_spec("api", 3102)],
    );

    let blog = registry.get("blog").expect("blog is registered");
    let lease = blog.acquire().await.expect("cold start succeeds");
    let process_before = blog.status().await.expect("manager is alive").process_id;

    // port가 겹치는 집합. app 하나는 멀쩡하지만 집합이 틀렸다.
    let error = registry
        .reload(vec![
            app_spec("blog", 3101),
            app_spec("api", 3102),
            app_spec("extra", 3101),
        ])
        .await
        .expect_err("a duplicate port must be rejected");
    assert!(
        matches!(
            error,
            nimwake_core::error::ValidationError::DuplicatePort { .. }
        ),
        "unexpected error: {error}",
    );

    // 기존 상태가 그대로다. 멀쩡했던 app조차 등록되지 않았다.
    assert_eq!(registry.len(), 2);
    assert!(registry.get("extra").is_none(), "nothing may be applied");

    let after = blog.status().await.expect("manager is alive");
    assert_eq!(after.state, AppStateName::Running);
    assert_eq!(after.process_id, process_before);
    assert_eq!(after.active_requests, 1);

    drop(lease);
    registry.shutdown().await;
}

/// 같은 집합으로 재적재하면 아무것도 바뀌지 않는다. (PRD §76)
///
/// `SIGHUP`은 운영자가 언제든 보낼 수 있다. 바뀐 것이 없을 때 전부 재시작하면
/// 신호를 한 번 더 보냈다는 이유로 서비스가 끊긴다.
#[tokio::test(start_paused = true)]
async fn reloading_the_same_set_is_a_no_op() {
    let runtime = Arc::new(FakeRuntime::default());
    let specs = vec![app_spec("blog", 3101), app_spec("api", 3102)];
    let registry = registry_of(&runtime, specs.clone());

    let blog = registry.get("blog").expect("blog is registered");
    let lease = blog.acquire().await.expect("cold start succeeds");
    let process_before = blog.status().await.expect("manager is alive").process_id;

    let outcome = registry
        .reload(specs)
        .await
        .expect("the same set validates");

    assert!(!outcome.changed(), "nothing may change: {outcome:?}");
    assert_eq!(outcome.unchanged, ["api", "blog"]);

    let after = blog.status().await.expect("manager is alive");
    assert_eq!(after.state, AppStateName::Running);
    assert_eq!(after.process_id, process_before);

    drop(lease);
    registry.shutdown().await;
}

// crash-loop backoff. (PRD §77)

/// 실패 3회 뒤에는 **기동을 시도하지 않는다.** (PRD §77)
///
/// 이 테스트가 §77의 전부다. backoff 판정이 맞아도 상태 기계가 그것을 쓰지
/// 않으면 아무것도 바뀌지 않는다. 그래서 `start_count`로 **spawn 횟수**를 본다.
#[tokio::test(start_paused = true)]
async fn repeated_failures_stop_spawning() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);

    // 계속 실패하게 만든다.
    runtime.fail_next_starts(usize::MAX);

    // 실패 3회. 셋 다 실제로 기동을 시도한다.
    for attempt in 1..=3 {
        let error = handle.acquire().await.expect_err("startup fails");
        assert!(
            matches!(error, AcquireError::StartFailed(_)),
            "시도 {attempt}회차는 시도한 실패여야 한다: {error}",
        );
    }
    assert_eq!(runtime.start_count(), 3, "세 번은 시도한다");

    // 네 번째는 시도하지 않는다.
    let error = handle.acquire().await.expect_err("backing off");
    match error {
        AcquireError::BackingOff { retry_after } => {
            assert!(
                retry_after > Duration::ZERO && retry_after <= Duration::from_secs(1),
                "첫 backoff는 1초다: {retry_after:?}",
            );
        }
        other => panic!("backoff여야 한다: {other}"),
    }
    assert_eq!(
        runtime.start_count(),
        3,
        "backoff 중에는 spawn하지 않는다 — 이것이 §77의 목적이다",
    );

    handle.shutdown().await;
}

/// 지연이 지나면 다시 시도한다. backoff는 영구 정지가 아니다. (PRD §77)
#[tokio::test(start_paused = true)]
async fn the_backoff_expires_and_the_next_request_retries() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);
    runtime.fail_next_starts(3);

    for _ in 0..3 {
        let _ = handle.acquire().await;
    }
    assert_eq!(runtime.start_count(), 3);

    // 지연 안에는 거절한다.
    assert!(
        matches!(handle.acquire().await, Err(AcquireError::BackingOff { .. })),
        "지연 안에는 거절한다",
    );

    // 지연이 지나면 시도하고, 이제 runtime은 성공한다.
    tokio::time::advance(Duration::from_secs(2)).await;
    let lease = handle.acquire().await.expect("지연이 지나면 다시 기동한다");
    assert_eq!(runtime.start_count(), 4);
    assert_eq!(
        handle.status().await.expect("manager is alive").state,
        AppStateName::Running
    );

    drop(lease);
    handle.shutdown().await;
}

/// 실패 1, 2회는 즉시 재시도한다. (PRD §77)
///
/// 일시적 실패까지 기다리게 만들면 "다음 요청이 앱을 깨운다"는 약속이 깨진다.
#[tokio::test(start_paused = true)]
async fn one_failure_does_not_delay_the_next_request() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);

    // 한 번만 실패한다.
    runtime.fail_next_starts(1);
    assert!(handle.acquire().await.is_err(), "첫 시도는 실패한다");

    // 시간을 보내지 않고 바로 다시 요청한다.
    let lease = handle
        .acquire()
        .await
        .expect("한 번의 실패는 기다리게 만들지 않는다");
    assert_eq!(runtime.start_count(), 2);

    drop(lease);
    handle.shutdown().await;
}

/// 창 밖으로 벌어진 실패는 backoff를 만들지 않는다. (PRD §77)
///
/// 하루를 정상으로 돌다가 한 번 죽은 application은 crash loop가 아니다.
#[tokio::test(start_paused = true)]
async fn failures_spread_beyond_the_window_never_back_off() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);
    runtime.fail_next_starts(usize::MAX);

    // 창(60초)보다 넓게 벌려 5회 실패시킨다.
    for _ in 0..5 {
        let error = handle.acquire().await.expect_err("startup fails");
        assert!(
            matches!(error, AcquireError::StartFailed(_)),
            "창 밖의 실패는 언제나 시도한 실패다: {error}",
        );
        tokio::time::advance(Duration::from_secs(120)).await;
    }

    assert_eq!(
        runtime.start_count(),
        5,
        "드문 실패는 전부 시도해야 한다 — spawn 속도가 이미 무해하다",
    );

    handle.shutdown().await;
}

/// 정상으로 돌다가 idle로 멈추면 이력이 초기화된다. (PRD §77)
#[tokio::test(start_paused = true)]
async fn a_successful_idle_cycle_clears_the_history() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 1);

    // 실패 2회를 쌓는다. 아직 임계값 아래다.
    runtime.fail_next_starts(2);
    for _ in 0..2 {
        let _ = handle.acquire().await;
    }

    // 성공하고 idle로 잠든다.
    let lease = handle.acquire().await.expect("세 번째는 성공한다");
    drop(lease);
    common::wait_for_active_requests(&handle, 0).await;
    tokio::time::advance(Duration::from_secs(2)).await;
    common::wait_for_state(&handle, AppStateName::Sleeping).await;

    // 이제 2회를 더 실패시킨다. 이력이 초기화됐으므로 backoff가 아니다.
    runtime.fail_next_starts(2);
    for attempt in 1..=2 {
        let error = handle.acquire().await.expect_err("startup fails");
        assert!(
            matches!(error, AcquireError::StartFailed(_)),
            "초기화 뒤 {attempt}회차는 시도한 실패여야 한다: {error}",
        );
    }

    handle.shutdown().await;
}

/// 기동은 성공하고 돌다가 죽는 application도 crash loop다. (PRD §77, §32)
///
/// 이 경로를 놓치면 가장 흔한 crash loop를 놓친다 — port를 열고 readiness를
/// 통과한 뒤 첫 요청에 죽는 앱이다. 기동은 매번 성공하므로 기동 실패만 세면
/// 영원히 backoff에 들어가지 않는다.
#[tokio::test(start_paused = true)]
async fn processes_that_die_after_starting_also_back_off() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);

    // 깨우고 죽이기를 3회. 기동은 매번 성공한다.
    for round in 1..=3 {
        let lease = handle
            .acquire()
            .await
            .unwrap_or_else(|error| panic!("{round}회차 기동은 성공해야 한다: {error}"));
        let process = handle
            .status()
            .await
            .expect("manager is alive")
            .process_id
            .expect("running has a process");

        drop(lease);
        runtime.crash(process);
        common::wait_for_state(&handle, AppStateName::Failed).await;
    }

    assert_eq!(runtime.start_count(), 3, "세 번 기동했다");

    // 네 번째 요청은 시도하지 않는다.
    let error = handle.acquire().await.expect_err("backing off");
    assert!(
        matches!(error, AcquireError::BackingOff { .. }),
        "비정상 종료도 실패로 세야 한다: {error}",
    );
    assert_eq!(runtime.start_count(), 3, "backoff 중에는 spawn하지 않는다",);

    handle.shutdown().await;
}

// 메모리 입장 제어. (PRD §78)

/// 입장 제어가 **꺼져 있으면 `/proc/meminfo`를 읽지 않는다.** (PRD §78)
///
/// 이것이 기본값의 비용이다. 요청마다 읽으면 lease 반납처럼 흔한 경로에
/// 비용이 붙는다. 기본으로 켜지 않기로 한 만큼, 꺼져 있을 때 공짜여야 한다.
#[tokio::test(start_paused = true)]
async fn the_check_costs_nothing_when_it_is_off() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 60);

    let lease = handle.acquire().await.expect("cold start succeeds");
    drop(lease);
    common::wait_for_active_requests(&handle, 0).await;
    let _second = handle.acquire().await.expect("warm request succeeds");

    assert_eq!(
        runtime.memory_reads(),
        0,
        "꺼져 있으면 한 번도 읽지 않아야 한다",
    );

    handle.shutdown().await;
}

/// 켜져 있으면 **깨우는 요청에서만** 읽는다. (PRD §78)
#[tokio::test(start_paused = true)]
async fn the_check_reads_only_when_waking() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_available_memory_mb(Some(10_000));
    let handle = app::spawn_with_limits(
        Arc::new(common::spec(60, 64)),
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        Limits {
            min_free_memory_mb: 500,
        },
    );

    // 깨우는 요청 하나 → 판독 1회.
    let lease = handle.acquire().await.expect("cold start succeeds");
    assert_eq!(runtime.memory_reads(), 1, "깨울 때 한 번 읽는다");

    // lease 반납은 Acquire가 아니다. 읽지 않는다.
    drop(lease);
    common::wait_for_active_requests(&handle, 0).await;
    assert_eq!(runtime.memory_reads(), 1, "lease 반납에는 읽지 않는다");

    // 이미 RUNNING이어도 Acquire이므로 읽는다. 결정 자체는 통과시킨다.
    let _warm = handle.acquire().await.expect("warm request succeeds");
    assert_eq!(runtime.memory_reads(), 2);

    handle.shutdown().await;
}

/// 메모리가 부족하면 실제로 **spawn하지 않는다.** (PRD §78)
///
/// 전이 테스트는 effect를 보고, 이쪽은 runtime의 `start_count`를 본다.
/// 거절만 하고 실제로는 spawn하는 것이 가장 나쁜 결과다.
#[tokio::test(start_paused = true)]
async fn a_short_system_does_not_spawn() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_available_memory_mb(Some(100));
    let handle = app::spawn_with_limits(
        Arc::new(common::spec(60, 64)),
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        Limits {
            min_free_memory_mb: 500,
        },
    );

    let error = handle.acquire().await.expect_err("memory is short");
    match error {
        AcquireError::InsufficientMemory {
            available_mb,
            required_mb,
        } => {
            assert_eq!(available_mb, 100);
            assert_eq!(required_mb, 500);
        }
        other => panic!("메모리 부족이어야 한다: {other}"),
    }
    assert_eq!(runtime.start_count(), 0, "spawn하지 않아야 한다");
    assert_eq!(
        handle.status().await.expect("manager is alive").state,
        AppStateName::Sleeping,
        "상태는 그대로다",
    );

    // 메모리가 풀리면 깨어난다.
    runtime.set_available_memory_mb(Some(800));
    let lease = handle.acquire().await.expect("memory is available now");
    assert_eq!(runtime.start_count(), 1);

    drop(lease);
    handle.shutdown().await;
}

// 유휴 시간 노출. eviction이 "가장 오래 유휴인 것"을 고르는 근거다. (PRD §80)

/// `idle_for`는 **마지막 요청**부터다. 기동부터가 아니다. (PRD §80)
///
/// 이것이 틀리면 오래 돌았지만 바쁜 application이 eviction 대상이 된다.
#[tokio::test(start_paused = true)]
async fn idle_for_measures_from_the_last_request_not_the_start() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 600);

    let lease = handle.acquire().await.expect("cold start succeeds");

    // 오래 돌게 둔다. 그동안 lease를 쥐고 있다 — 바쁜 app이다.
    tokio::time::advance(Duration::from_secs(300)).await;
    let busy = handle.status().await.expect("manager is alive");
    assert_eq!(busy.active_requests, 1);
    assert!(
        busy.since.expect("running has uptime") >= Duration::from_secs(300),
        "기동부터는 300초가 지났다",
    );

    // lease를 반납하면 그 순간이 마지막 요청이다.
    drop(lease);
    common::wait_for_active_requests(&handle, 0).await;

    let after = handle.status().await.expect("manager is alive");
    assert!(
        after.idle_for.expect("running has idle_for") < Duration::from_secs(1),
        "마지막 요청 직후이므로 유휴 시간은 0에 가깝다: {:?}",
        after.idle_for,
    );
    assert!(
        after.since.expect("running has uptime") >= Duration::from_secs(300),
        "기동부터는 여전히 300초다 — 둘은 다른 값이다",
    );

    // 시간이 지나면 유휴 시간만 자란다.
    tokio::time::advance(Duration::from_secs(120)).await;
    let later = handle.status().await.expect("manager is alive");
    assert!(
        later.idle_for.expect("running has idle_for") >= Duration::from_secs(120),
        "유휴 시간이 자라야 한다: {:?}",
        later.idle_for,
    );

    handle.shutdown().await;
}

/// 잠든 application에는 유휴 시간이 없다. (PRD §80)
///
/// eviction 후보는 `RUNNING`뿐이다. 잠든 것은 이미 메모리를 쓰지 않는다.
#[tokio::test(start_paused = true)]
async fn a_sleeping_app_has_no_idle_time() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 1);

    let status = handle.status().await.expect("manager is alive");
    assert_eq!(status.state, AppStateName::Sleeping);
    assert!(status.idle_for.is_none(), "잠든 app에는 유휴 시간이 없다");

    handle.shutdown().await;
}

/// 메모리 때문에 내린 것은 `Idle`과 구분된다. (PRD §80)
#[tokio::test(start_paused = true)]
async fn memory_pressure_is_a_distinct_stop_reason() {
    let runtime = Arc::new(FakeRuntime::default());
    let handle = spawn_app(&runtime, 600);

    let lease = handle.acquire().await.expect("cold start succeeds");
    drop(lease);
    common::wait_for_active_requests(&handle, 0).await;

    handle
        .stop(nimwake_core::StopReason::MemoryPressure)
        .await
        .expect("the app accepts the stop");
    common::wait_for_state(&handle, AppStateName::Sleeping).await;
    assert_eq!(runtime.alive_processes(), 0, "process가 사라져야 한다");

    // 이름이 달라야 로그에서 가를 수 있다.
    assert_eq!(
        nimwake_core::StopReason::MemoryPressure.to_string(),
        "memory pressure",
    );
    assert_ne!(
        nimwake_core::StopReason::MemoryPressure.to_string(),
        nimwake_core::StopReason::Idle.to_string(),
    );

    handle.shutdown().await;
}

// eviction sweep — 압박부터 정지까지. (PRD §80)

fn limits_mb(mb: u64) -> Limits {
    Limits {
        min_free_memory_mb: mb,
    }
}

/// eviction 테스트용 spec. **idle timeout을 길게 둔다.**
///
/// `app_spec`의 60초로는 유휴 시간을 벌리는 동안 app이 스스로 잠들어 후보에서
/// 빠진다. 실제로 그렇게 한 번 틀렸다 — 보려던 것은 eviction의 선택이고
/// idle timeout이 아니다.
fn long_lived_spec(name: &str, port: u16) -> nimwake_core::app::AppSpec {
    let mut spec = common::spec(3600, 64);
    spec.name = name.to_owned();
    spec.server.host = format!("{name}.localhost");
    spec.server.port = port;
    spec
}

/// **메모리가 부족하면 가장 오래 유휴인 것을 내린다.** (PRD §80)
///
/// 이 테스트가 §80의 전부다. 압박 판단 · 후보 선택 · 정지가 한 줄로 이어진다.
#[tokio::test(start_paused = true)]
async fn a_sweep_evicts_the_longest_idle_app() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_available_memory_mb(Some(10_000));
    let registry = AppRegistry::build(
        vec![
            long_lived_spec("older", 3101),
            long_lived_spec("fresher", 3102),
        ],
        DEFAULT_GATEWAY_PORT,
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        limits_mb(500),
    )
    .expect("registry builds");
    let shared: Arc<dyn Runtime> = Arc::clone(&runtime) as Arc<dyn Runtime>;

    // older를 먼저 깨우고 유휴로 둔다.
    let older = registry.get("older").expect("older is registered");
    let lease = older.acquire().await.expect("cold start succeeds");
    drop(lease);
    common::wait_for_active_requests(&older, 0).await;

    // 시간을 보내 older의 유휴 시간을 벌린다.
    tokio::time::advance(Duration::from_secs(120)).await;

    // 이제 fresher를 깨운다 — 유휴 시간이 짧다.
    let fresher = registry.get("fresher").expect("fresher is registered");
    let lease = fresher.acquire().await.expect("cold start succeeds");
    drop(lease);
    common::wait_for_active_requests(&fresher, 0).await;

    // 넉넉하면 아무것도 내리지 않는다.
    let evicted = eviction::sweep(&registry, &shared, limits_mb(500))
        .await
        .expect("sweep succeeds");
    assert_eq!(evicted, None, "메모리가 넉넉하면 내리지 않는다");

    // 메모리가 마르면 **older**가 내려가야 한다.
    runtime.set_available_memory_mb(Some(100));
    let evicted = eviction::sweep(&registry, &shared, limits_mb(500))
        .await
        .expect("sweep succeeds");
    assert_eq!(
        evicted.as_deref(),
        Some("older"),
        "가장 오래 유휴인 것을 내려야 한다",
    );

    common::wait_for_state(&older, AppStateName::Sleeping).await;
    assert_eq!(
        fresher.status().await.expect("manager is alive").state,
        AppStateName::Running,
        "최근에 쓴 것은 그대로 둔다",
    );

    registry.shutdown().await;
}

/// **활성 요청이 있으면 내리지 않는다.** (PRD §80)
///
/// 더 오래 유휴여도 바쁜 쪽을 고르면 lease를 쥔 요청이 깨진다.
#[tokio::test(start_paused = true)]
async fn a_sweep_never_evicts_a_serving_app() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_available_memory_mb(Some(10_000));
    let registry = AppRegistry::build(
        vec![long_lived_spec("busy", 3101)],
        DEFAULT_GATEWAY_PORT,
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        limits_mb(500),
    )
    .expect("registry builds");
    let shared: Arc<dyn Runtime> = Arc::clone(&runtime) as Arc<dyn Runtime>;

    // lease를 **쥔 채로** 둔다 — 요청을 처리하는 중이다.
    let busy = registry.get("busy").expect("busy is registered");
    let lease = busy.acquire().await.expect("cold start succeeds");
    tokio::time::advance(Duration::from_secs(9999)).await;

    runtime.set_available_memory_mb(Some(1));
    let evicted = eviction::sweep(&registry, &shared, limits_mb(500))
        .await
        .expect("sweep succeeds");
    assert_eq!(evicted, None, "바쁜 app을 내리면 안 된다");
    assert_eq!(
        busy.status().await.expect("manager is alive").state,
        AppStateName::Running,
    );

    drop(lease);
    registry.shutdown().await;
}

/// 임계값이 `0`(기본)이면 메모리를 읽지도 않는다. (PRD §78, §80)
#[tokio::test(start_paused = true)]
async fn a_sweep_is_free_when_the_threshold_is_off() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_available_memory_mb(Some(1));
    let registry = AppRegistry::build(
        vec![long_lived_spec("blog", 3101)],
        DEFAULT_GATEWAY_PORT,
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        Limits::default(),
    )
    .expect("registry builds");
    let shared: Arc<dyn Runtime> = Arc::clone(&runtime) as Arc<dyn Runtime>;

    let blog = registry.get("blog").expect("blog is registered");
    let lease = blog.acquire().await.expect("cold start succeeds");
    drop(lease);
    common::wait_for_active_requests(&blog, 0).await;

    let reads_before = runtime.memory_reads();
    let evicted = eviction::sweep(&registry, &shared, Limits::default())
        .await
        .expect("sweep succeeds");
    assert_eq!(evicted, None);
    assert_eq!(
        runtime.memory_reads(),
        reads_before,
        "꺼져 있으면 메모리를 읽지 않는다",
    );

    registry.shutdown().await;
}

/// 가용량을 읽을 수 없으면 아무것도 내리지 않는다. (PRD §78, §80)
#[tokio::test(start_paused = true)]
async fn an_unknown_reading_evicts_nothing() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_available_memory_mb(Some(10_000));
    let registry = AppRegistry::build(
        vec![long_lived_spec("blog", 3101)],
        DEFAULT_GATEWAY_PORT,
        Arc::clone(&runtime) as Arc<dyn Runtime>,
        limits_mb(500),
    )
    .expect("registry builds");
    let shared: Arc<dyn Runtime> = Arc::clone(&runtime) as Arc<dyn Runtime>;

    let blog = registry.get("blog").expect("blog is registered");
    let lease = blog.acquire().await.expect("cold start succeeds");
    drop(lease);
    common::wait_for_active_requests(&blog, 0).await;

    runtime.set_available_memory_mb(None);
    let evicted = eviction::sweep(&registry, &shared, limits_mb(500))
        .await
        .expect("sweep succeeds");
    assert_eq!(evicted, None, "읽을 수 없으면 내리지 않는다");
    assert_eq!(
        blog.status().await.expect("manager is alive").state,
        AppStateName::Running,
    );

    registry.shutdown().await;
}
