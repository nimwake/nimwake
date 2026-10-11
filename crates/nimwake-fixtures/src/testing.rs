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

/// 이미 내준 port. 같은 process 안에서 두 번 내주지 않기 위한 것이다.
static HANDED_OUT: std::sync::Mutex<std::collections::BTreeSet<u16>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// port 예약 파일이 놓이는 곳.
fn reservation_dir() -> std::path::PathBuf {
    std::env::temp_dir().join("nimwake-test-ports")
}

/// port를 이 process의 것으로 예약한다. 이미 살아 있는 다른 process의 것이면 `false`.
///
/// `cargo test`는 테스트 binary를 **병렬 process로** 돌린다. `HANDED_OUT`은 한
/// process 안에서만 유효하고, bind 검사는 "지금 비어 있다"만 말해 준다. 그래서
/// 두 binary가 거의 동시에 같은 번호를 받아 가는 창이 남는다.
///
/// 파일을 `create_new`로 만들어 그 창을 닫는다. 커널이 중재하므로 경쟁이 없다.
/// 죽은 process가 남긴 예약은 회수한다. SIGKILL이나 panic으로 끝나면 파일이 남는데,
/// 그것을 영구 점유로 두면 다음 실행이 port를 잃는다.
fn reserve_port(port: u16) -> bool {
    use std::io::ErrorKind;

    let dir = reservation_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("{port}"));

    let pid = std::process::id();
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => {
            let _ = file.write_all(pid.to_string().as_bytes());
            true
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            let owner = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok());

            match owner {
                // 우리 것이면 이미 예약돼 있다.
                Some(owner) if owner == pid => true,
                // 임자가 죽었으면 회수한다.
                Some(owner) if !pid_is_alive(owner) => {
                    let _ = std::fs::remove_file(&path);
                    std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map(|mut file| {
                            let _ = file.write_all(pid.to_string().as_bytes());
                            true
                        })
                        .unwrap_or(false)
                }
                // 살아 있는 남의 것, 또는 읽을 수 없는 예약.
                _ => false,
            }
        }
        Err(_) => false,
    }
}

/// 예약 파일의 임자가 살아 있는지 본다.
///
/// [`is_alive`]와 달리 Unix 밖에서도 컴파일돼야 한다. 그쪽에서는 알 수 없으므로
/// 살아 있다고 본다. 회수를 못 하는 편이 남의 port를 빼앗는 것보다 안전하다.
#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    is_alive(pid)
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
}

/// 비어 있는 loopback port를 고른다.
///
/// 세 겹으로 막는다.
///
/// ```text
/// HANDED_OUT      같은 process 안에서 두 번 내주지 않는다
/// 예약 파일        다른 process와 겹치지 않는다 (커널이 중재한다)
/// bind 검사        지금 실제로 비어 있는지 확인한다
/// ```
///
/// 세 번째만으로는 부족했다. `cargo test`가 테스트 binary를 병렬로 돌리므로,
/// 두 binary가 거의 동시에 같은 번호를 받아 한쪽의 readiness probe가 다른 쪽
/// fixture에 붙었다. 처음에는 `crash-server`의 "기동 실패" 단언이 깨지는 모습으로,
/// 그다음에는 `stop_terminates_the_whole_process_group`의 기동이 "port가 이미
/// 쓰이고 있다"로 거부되는 모습으로 나타났다. 같은 원인이다.
///
/// # Panics
///
/// 범위 안에서 빈 port를 찾지 못하면 panic한다.
#[must_use]
pub fn free_port() -> u16 {
    let seed = u16::try_from(std::process::id() % u32::from(PORT_SPAN)).unwrap_or(0);
    let offset = NEXT_PORT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut handed_out = HANDED_OUT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    for attempt in 0..PORT_SPAN {
        let port = PORT_BASE + seed.wrapping_add(offset).wrapping_add(attempt) % PORT_SPAN;

        if handed_out.contains(&port) {
            continue;
        }
        if !reserve_port(port) {
            continue;
        }
        if TcpListener::bind(("127.0.0.1", port)).is_err() {
            continue;
        }

        // bind가 된다는 것과 "connect가 거부된다"는 것은 같은 뜻이 아니다.
        //
        // `ProcessRuntime::start`는 spawn 전에 connect로 port가 비었는지 본다.
        // 그 기준으로 확인하지 않으면 bind는 되는데 start는 거부되는 port를
        // 내주게 되고, 테스트가 간헐적으로 "already in use"로 깨진다.
        //
        // 실제로 그랬다. 50ms로 훑어도 LISTEN은 한 번도 보이지 않는데 connect는
        // 성공하는 순간이 있었다. 무엇이 응답하는지는 특정하지 못했다.
        // 특정하지 못한 것을 추측해 두는 대신, **쓰는 쪽과 같은 기준으로 확인한다.**
        if TcpStream::connect_timeout(&addr(port), ALLOCATION_PROBE_TIMEOUT).is_ok() {
            continue;
        }

        handed_out.insert(port);
        return port;
    }

    panic!("no free port in {PORT_BASE}..{}", PORT_BASE + PORT_SPAN);
}

/// 할당 직전 확인에 쓰는 connect 제한.
///
/// 비어 있는 loopback port는 기다리지 않고 곧바로 `ECONNREFUSED`로 답하므로
/// 짧게 두어도 느려지지 않는다.
const ALLOCATION_PROBE_TIMEOUT: Duration = Duration::from_millis(100);

#[must_use]
pub fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// loopback port에 연결이 되는지 본다. readiness와 같은 판정 기준이다. (PRD §22)
///
/// timeout이 넉넉한 것은 **느린 CI runner에서 살아 있는 port를 죽은 것으로 읽지 않기** 위해서다.
/// 200ms로 두었더니 `acquire()`가 성공한 직후의 단언이 CI에서 간헐적으로 깨졌다.
///
/// 이 값을 키워도 "닫혀 있음"을 확인하는 쪽은 느려지지 않는다.
/// loopback의 닫힌 port는 timeout을 기다리지 않고 곧바로 `ECONNREFUSED`로 답한다.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

#[must_use]
pub fn is_listening(port: u16) -> bool {
    TcpStream::connect_timeout(&addr(port), CONNECT_TIMEOUT).is_ok()
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
    /// 응답 header. 이름은 소문자로 맞춰 둔다. HTTP header 이름은 대소문자를
    /// 구분하지 않으므로 그대로 두면 테스트가 서버의 표기에 묶인다.
    pub headers: Vec<(String, String)>,
}

impl HttpResponse {
    /// header 하나를 이름으로 찾는다. 이름은 대소문자를 구분하지 않는다.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<String> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
    }

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
    request_within(
        gateway_port,
        method,
        path,
        host,
        body,
        Duration::from_secs(30),
    )
}

/// [`request`]와 같되 읽기 제한을 지정한다.
///
/// 응답하지 않는 backend를 검증할 때 쓴다. 기본 30초를 기다릴 이유가 없다.
///
/// # Errors
///
/// [`request`]와 같다. 제한을 넘기면 `WouldBlock` 또는 `TimedOut`이다.
///
/// # Panics
///
/// [`request`]와 같다.
pub fn request_within(
    gateway_port: u16,
    method: &str,
    path: &str,
    host: &str,
    body: Option<&str>,
    read_timeout: Duration,
) -> std::io::Result<HttpResponse> {
    let mut stream = TcpStream::connect_timeout(&addr(gateway_port), Duration::from_secs(2))?;
    stream.set_read_timeout(Some(read_timeout))?;

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

    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();

    Ok(HttpResponse {
        status,
        body: body.to_owned(),
        headers,
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
