//! 통합 테스트 공통 도구.
//!
//! HTTP client와 port 할당은 `nimwake_fixtures::testing`에 있다. `nimwaked`의 E2E와
//! 같은 것을 쓰기 위해서다. 여기에는 `AppSpec` 조립과 async 대기만 남긴다.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use nimwake_core::app::{AppSpec, LifecycleSpec, RuntimeSpec, ServerSpec};

// 테스트 binary마다 쓰는 것이 다르다. 이 모듈은 각 binary에 따로 컴파일되므로
// 어느 쪽에서 보든 나머지는 미사용으로 보인다.
#[allow(unused_imports)]
pub use nimwake_fixtures::testing::{
    addr, free_port, get, is_alive, is_listening, request, request_within, HttpResponse,
};

/// fixture 하나를 실행하는 `AppSpec`을 만든다.
pub fn spec(name: &str, port: u16, command: Vec<String>) -> AppSpec {
    AppSpec {
        name: name.to_owned(),
        server: ServerSpec {
            host: format!("{name}.localhost"),
            // §87의 alias. 이 fixture는 쓰지 않는다.
            aliases: Vec::new(),
            port,
            // 기본은 모두 허용. peer 제한을 보는 테스트가 직접 채운다. (PRD §82)
            allow: Vec::new(),
        },
        runtime: RuntimeSpec {
            command,
            working_dir: PathBuf::from("."),
            env: BTreeMap::new(),
        },
        lifecycle: LifecycleSpec::default(),
    }
}

/// app별 환경과 작업 디렉터리를 지정한다. (PRD §20)
pub fn with_runtime(mut spec: AppSpec, working_dir: PathBuf, env: &[(&str, &str)]) -> AppSpec {
    spec.runtime.working_dir = working_dir;
    spec.runtime.env = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    spec
}

/// pending waiter 상한을 바꾼다. (PRD §69)
pub fn with_pending_limit(mut spec: AppSpec, limit: usize) -> AppSpec {
    spec.lifecycle.max_pending_requests = limit;
    spec
}

/// `spec`의 lifecycle 타이밍을 테스트 길이에 맞게 줄인다.
pub fn with_timeouts(mut spec: AppSpec, startup: u64, shutdown: u64, idle: u64) -> AppSpec {
    spec.lifecycle.startup_timeout_seconds = startup;
    spec.lifecycle.shutdown_timeout_seconds = shutdown;
    spec.lifecycle.idle_timeout_seconds = idle;
    spec
}

/// 응답 header 상한만 바꾼다. 기본값 60초를 테스트에서 기다릴 이유가 없다. (PRD §75)
pub fn with_response_header_timeout(mut spec: AppSpec, seconds: u64) -> AppSpec {
    spec.lifecycle.response_header_timeout_seconds = seconds;
    spec
}

/// HTTP 요청 하나를 보내고 응답 첫 줄을 돌려준다.
pub fn http_get(port: u16) -> std::io::Result<String> {
    Ok(format!(
        "HTTP/1.1 {} OK",
        get(port, "test.localhost")?.status
    ))
}

/// 조건이 참이 될 때까지 기다린다. 기한을 넘기면 `false`.
pub async fn eventually(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    condition()
}

/// 조건이 참이 될 때까지 기다리고, 끝내 아니면 실패시킨다.
pub async fn wait_for(what: &str, deadline: Duration, condition: impl FnMut() -> bool) {
    assert!(
        eventually(deadline, condition).await,
        "timed out after {deadline:?} waiting for {what}",
    );
}

pub mod bin {
    pub const ECHO: &str = env!("CARGO_BIN_EXE_echo-server");
    pub const SLOW_START: &str = env!("CARGO_BIN_EXE_slow-start-server");
    pub const CRASH: &str = env!("CARGO_BIN_EXE_crash-server");
    pub const NOISY: &str = env!("CARGO_BIN_EXE_noisy-server");
    pub const STUBBORN: &str = env!("CARGO_BIN_EXE_stubborn-server");
    pub const EXIT_AFTER: &str = env!("CARGO_BIN_EXE_exit-after-server");
    pub const GROUP: &str = env!("CARGO_BIN_EXE_group-server");
    pub const DEAF: &str = env!("CARGO_BIN_EXE_deaf-server");
    pub const MUTE: &str = env!("CARGO_BIN_EXE_mute-server");
    pub const SLOW_BODY: &str = env!("CARGO_BIN_EXE_slow-body-server");
}

/// `command` 벡터를 만든다.
pub fn cmd(program: &str, args: &[String]) -> Vec<String> {
    let mut command = vec![program.to_owned()];
    command.extend(args.iter().cloned());
    command
}
