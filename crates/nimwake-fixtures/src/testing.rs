//! 통합 테스트가 공유하는 도구.
//!
//! `nimwake-fixtures`의 테스트와 `nimwaked`의 E2E가 같은 것을 쓴다.
//! 두 패키지에 같은 HTTP client를 두 벌 두지 않기 위해 라이브러리로 올렸다.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

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
///
/// # Panics
///
/// 범위 안에서 빈 port를 찾지 못하면 panic한다.
#[must_use]
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

#[must_use]
pub fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// loopback port에 연결이 되는지 본다. readiness와 같은 판정 기준이다. (PRD §22)
#[must_use]
pub fn is_listening(port: u16) -> bool {
    TcpStream::connect_timeout(&addr(port), Duration::from_millis(200)).is_ok()
}

/// pid가 아직 살아 있는지 본다. signal 0은 권한 검사만 수행한다.
///
/// `pgrep -f`로 확인하면 검사 스크립트 자신이 패턴에 걸려 오탐이 난다. 늘 pid로 본다.
///
/// # Panics
///
/// pid가 `i32` 범위를 벗어나면 panic한다.
#[cfg(unix)]
#[must_use]
pub fn is_alive(pid: u32) -> bool {
    let pid = nix::unistd::Pid::from_raw(i32::try_from(pid).expect("pid fits in i32"));
    nix::sys::signal::kill(pid, None).is_ok()
}

/// 조건이 참이 될 때까지 기다린다. 기한을 넘기면 `false`.
///
/// 동기 함수다. async 문맥에서 부를 때는 `spawn_blocking`으로 감싼다.
pub fn eventually(deadline: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    condition()
}

/// 조건이 참이 될 때까지 기다리고, 끝내 아니면 실패시킨다.
///
/// # Panics
///
/// 기한 안에 조건이 참이 되지 않으면 panic한다.
pub fn wait_for(what: &str, deadline: Duration, condition: impl FnMut() -> bool) {
    assert!(
        eventually(deadline, condition),
        "timed out after {deadline:?} waiting for {what}",
    );
}

/// gateway로 보낸 요청의 응답.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

impl HttpResponse {
    /// fixture는 pid를 body로 돌려준다. (`serve_forever`)
    ///
    /// # Panics
    ///
    /// body가 pid가 아니면 panic한다.
    #[must_use]
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
///
/// # Errors
///
/// 연결과 송수신 실패를 그대로 돌려준다.
///
/// # Panics
///
/// 응답이 HTTP 형식이 아니면 panic한다. 테스트에서는 그것이 올바른 실패다.
pub fn request(
    gateway_port: u16,
    method: &str,
    path: &str,
    host: &str,
    body: Option<&str>,
) -> std::io::Result<HttpResponse> {
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
///
/// # Errors
///
/// [`request`]와 같다.
pub fn get(gateway_port: u16, host: &str) -> std::io::Result<HttpResponse> {
    request(gateway_port, "GET", "/", host, None)
}
