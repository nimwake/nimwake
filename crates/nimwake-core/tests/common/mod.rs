//! 통합 테스트 공용 헬퍼.
//!
//! 이 모듈은 각 테스트 바이너리에 개별적으로 컴파일되므로,
//! 한 바이너리에서 쓰지 않는 헬퍼가 있는 것이 정상이다.

#![allow(dead_code)]

use std::time::Duration;

use nimwake_core::app::AppSpec;
use nimwake_core::{AppHandle, AppStateName};

pub fn spec(idle_seconds: u64, max_pending_requests: usize) -> AppSpec {
    AppSpec::from_toml(&format!(
        r#"
name = "test"

[server]
host = "test.localhost"
port = 3101

[runtime]
command = ["true"]
working_dir = "."

[lifecycle]
idle_timeout_seconds = {idle_seconds}
startup_timeout_seconds = 5
shutdown_timeout_seconds = 5
max_pending_requests = {max_pending_requests}
"#
    ))
    .expect("test spec parses")
}

/// 목표 상태가 될 때까지 기다린다.
///
/// `sleep`으로 폴링하는 이유: `time::pause` 상태에서 runtime이 idle해져야
/// 가상 시계가 자동으로 흐른다. `yield_now`로 돌면 시계가 멈춘 채 굶는다.
pub async fn wait_for_state(handle: &AppHandle, target: AppStateName) {
    for _ in 0..200 {
        let status = handle.status().await.expect("manager is alive");
        if status.state == target {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let status = handle.status().await.expect("manager is alive");
    panic!("state never reached {target}, still {}", status.state);
}

/// 살아 있는 process가 0이 될 때까지 기다린다.
pub async fn wait_for_zero_alive(runtime: &std::sync::Arc<nimwake_core::runtime::FakeRuntime>) {
    for _ in 0..200 {
        if runtime.alive_processes() == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "processes never terminated, {} still alive",
        runtime.alive_processes()
    );
}

/// active request 수가 목표값이 될 때까지 기다린다.
pub async fn wait_for_active_requests(handle: &AppHandle, target: usize) {
    for _ in 0..200 {
        let status = handle.status().await.expect("manager is alive");
        if status.active_requests == target {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let status = handle.status().await.expect("manager is alive");
    panic!(
        "active_requests never reached {target}, still {}",
        status.active_requests
    );
}
