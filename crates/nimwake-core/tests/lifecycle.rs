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
