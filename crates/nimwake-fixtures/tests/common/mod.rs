//! 통합 테스트 공통 도구.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use nimwake_core::app::{AppSpec, LifecycleSpec, RuntimeSpec, ServerSpec};

/// ephemeral range 아래쪽 port 번호의 시작점.
///
/// `bind(:0)`으로 고르면 안 된다. 돌려준 직후 커널이 같은 번호를 **다른 연결의 ephemeral
/// port로** 내줄 수 있고, 그러면 readiness probe가 엉뚱한 listener에 붙는다.
/// 실제로 그 경로에서 테스트가 간헐적으로 깨졌다.
const PORT_BASE: u16 = 20_000;
const PORT_SPAN: u16 = 10_000;

static NEXT_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

/// 비어 있는 loopback port를 고른다.
///
/// 같은 binary 안의 테스트끼리는 원자 counter로, 다른 binary와는 pid로 갈린다.
/// 실제로 bind되는지까지 확인하고 돌려준다.
pub fn free_port() -> u16 {
    let seed = u16::try_from(std::process::id() % u32::from(PORT_SPAN)).unwrap_or(0);
    let offset = NEXT_PORT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    for attempt in 0..PORT_SPAN {
        let port = PORT_BASE + seed.wrapping_add(offset).wrapping_add(attempt) % PORT_SPAN;
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }

    panic!("no free port in {PORT_BASE}..{}", PORT_BASE + PORT_SPAN);
}

/// fixture 하나를 실행하는 `AppSpec`을 만든다.
pub fn spec(name: &str, port: u16, command: Vec<String>) -> AppSpec {
    AppSpec {
        name: name.to_owned(),
        server: ServerSpec {
            host: format!("{name}.localhost"),
            port,
        },
        runtime: RuntimeSpec {
            command,
            working_dir: PathBuf::from("."),
            env: BTreeMap::new(),
        },
        lifecycle: LifecycleSpec::default(),
    }
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

pub fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// loopback port에 연결이 되는지 본다. readiness와 같은 판정 기준이다. (PRD §22)
pub fn is_listening(port: u16) -> bool {
    TcpStream::connect_timeout(&addr(port), Duration::from_millis(200)).is_ok()
}

/// HTTP 요청 하나를 보내고 응답 첫 줄을 돌려준다.
pub fn http_get(port: u16) -> std::io::Result<String> {
    use std::io::{BufRead, BufReader, Write};

    let mut stream = TcpStream::connect_timeout(&addr(port), Duration::from_secs(1))?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"GET / HTTP/1.1\r\nhost: test.localhost\r\nconnection: close\r\n\r\n")?;
    stream.flush()?;

    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line)
}

/// pid가 아직 살아 있는지 본다. signal 0은 권한 검사만 수행한다.
pub fn is_alive(pid: u32) -> bool {
    let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits in i32"));
    nix::sys::signal::kill(pid, None).is_ok()
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
    pub const SLOW_BODY: &str = env!("CARGO_BIN_EXE_slow-body-server");
}

/// `command` 벡터를 만든다.
pub fn cmd(program: &str, args: &[String]) -> Vec<String> {
    let mut command = vec![program.to_owned()];
    command.extend(args.iter().cloned());
    command
}

/// gateway로 보낸 요청의 응답.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    /// fixture는 pid를 body로 돌려준다. (`serve_forever`)
    pub fn pid(&self) -> u32 {
        self.body
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("expected a pid body, got {:?}", self.body))
    }
}

/// gateway에 요청 하나를 보낸다.
///
/// `connection: close`로 보내므로 응답은 EOF까지 읽으면 된다.
/// 테스트 전용이라 chunked encoding은 다루지 않는다. fixture는 항상 content-length를 준다.
pub fn request(
    gateway_port: u16,
    method: &str,
    path: &str,
    host: &str,
    body: Option<&str>,
) -> std::io::Result<HttpResponse> {
    use std::io::{Read, Write};

    let mut stream = TcpStream::connect_timeout(&addr(gateway_port), Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;

    let body = body.unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nhost: {host}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    let raw = String::from_utf8_lossy(&raw).into_owned();

    let (head, body) = raw
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("malformed response: {raw:?}"));

    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status code in: {head:?}"));

    Ok(HttpResponse {
        status,
        body: body.to_owned(),
    })
}

/// `GET /` 요청 하나. 가장 흔한 형태라 따로 둔다.
pub fn get(gateway_port: u16, host: &str) -> std::io::Result<HttpResponse> {
    request(gateway_port, "GET", "/", host, None)
}
