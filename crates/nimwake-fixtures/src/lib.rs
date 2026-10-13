//! `ProcessRuntime` / Gateway 통합 테스트용 fixture 서버들의 공통 코드.
//!
//! 요구사항: `.docs/phases/phase-1/03-milestones/02-v0.0.2-process-runtime.md`,
//! `.docs/phases/phase-1/03-milestones/03-v0.0.3-http-scale-to-zero.md`
//!
//! 의존성을 최소로 유지한다. fixture가 복잡해지면 테스트가 무엇을 증명하는지 흐려진다.

pub mod testing;

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// argv에서 위치 인자를 읽는다.
///
/// # Panics
///
/// 인자가 없거나 형식이 맞지 않으면 panic한다. fixture에서는 그것이 올바른 실패다.
#[must_use]
pub fn arg<T: std::str::FromStr>(index: usize, name: &str) -> T {
    let raw = std::env::args()
        .nth(index)
        .unwrap_or_else(|| panic!("missing argument {index} ({name})"));
    raw.parse()
        .unwrap_or_else(|_| panic!("argument {index} ({name}) is not valid: `{raw}`"))
}

/// loopback에 bind한다. 이 시점부터 readiness probe가 성공한다. (PRD §22)
///
/// # Panics
///
/// bind에 실패하면 panic한다.
#[must_use]
pub fn listen(port: u16) -> TcpListener {
    TcpListener::bind(("127.0.0.1", port))
        .unwrap_or_else(|error| panic!("failed to bind 127.0.0.1:{port}: {error}"))
}

/// 최소 HTTP/1.1 서버. 정상 종료 경로는 없다.
///
/// 응답 규칙:
///
/// ```text
/// POST /echo   요청 body를 그대로 돌려준다   (cold start를 거친 body 전달 검증)
/// GET /cwd     자기 작업 디렉터리를 돌려준다   (app별 분리 검증, PRD §20)
/// GET /env/X   환경변수 X의 값을 돌려준다      (daemon 환경 overlay 검증, PRD §20)
/// 그 외        자기 pid를 돌려준다            (same PID / new PID 관측)
/// ```
pub fn serve_forever(listener: &TcpListener) -> ! {
    loop {
        if let Ok((stream, _)) = listener.accept() {
            // 한 연결의 실패가 서버를 멈추게 하지 않는다.
            let _ = respond(stream);
        }
    }
}

/// 읽어 들인 요청.
pub struct Request {
    pub line: String,
    /// 받은 header를 온 순서대로. `header-echo-server`가 이것을 돌려준다.
    ///
    /// gateway가 넣은 것(`X-Forwarded-For` 등)을 application이 **실제로 받는지**
    /// 확인하려면 받은 쪽에서 되돌려 줘야 한다. (PRD §81)
    pub headers: Vec<String>,
    pub body: Vec<u8>,
}

/// 요청 한 건을 읽는다. 연결이 곧바로 닫혔으면 `None`.
///
/// **응답 전에 반드시 이것을 부른다.** readiness probe는 아무것도 보내지 않고 연결을 닫는데,
/// 읽지 않고 응답부터 쓰면 그 죽은 연결을 붙들고 있는 동안 진짜 요청이 backlog에서 기다린다.
///
/// # Errors
///
/// 소켓 읽기가 실패하면 그대로 돌려준다.
pub fn read_request<R: BufRead>(reader: &mut R) -> std::io::Result<Option<Request>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }

    let mut content_length = 0usize;
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some(value) = header_value(header, "content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        headers.push(header.to_owned());
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        std::io::Read::read_exact(reader, &mut body)?;
    }

    Ok(Some(Request {
        line,
        headers,
        body,
    }))
}

fn respond(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };

    let payload = if request.line.starts_with("POST /echo") {
        request.body
    } else if request.line.starts_with("GET /cwd") {
        // app별 working directory 분리를 밖에서 관측할 수 있게 한다. (PRD §20)
        std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
            .into_bytes()
    } else if let Some(name) = env_query(&request.line) {
        // `[runtime.env]`가 daemon 환경을 **덮지 않고 올려 쓰는지** 보기 위한 것이다.
        // 그래서 app이 지정하지 않은 변수(PATH 등)도 물어볼 수 있어야 한다. (PRD §20)
        std::env::var(name).unwrap_or_default().into_bytes()
    } else {
        std::process::id().to_string().into_bytes()
    };

    let mut stream = reader.into_inner();
    write!(
            stream,
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n",
        payload.len()
    )?;
    stream.write_all(&payload)?;
    stream.flush()
}

/// `GET /env/NAME` 요청에서 `NAME`을 뽑는다.
fn env_query(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("GET /env/")?;
    let name = rest.split_whitespace().next()?;
    (!name.is_empty()).then_some(name)
}

fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    key.trim().eq_ignore_ascii_case(name).then_some(value)
}
