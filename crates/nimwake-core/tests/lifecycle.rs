//! 공개 API로 본 lifecycle 전체 경로.
//!
//! 요구사항: PRD §10 Application Lifecycle, §27 Scale-to-Zero, §32–§33.
//!
//! 가상 시계(`start_paused`)를 사용해 idle timeout을 결정적으로 재현한다.

mod common;

use std::sync::Arc;
use std::time::Duration;

use nimwake_core::app;
use nimwake_core::registry::{self, AppRegistry, DEFAULT_GATEWAY_PORT};
use nimwake_core::runtime::FakeRuntime;
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
