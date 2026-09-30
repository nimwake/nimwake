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
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        std::io::Read::read_exact(reader, &mut body)?;
    }

    Ok(Some(Request { line, body }))
}

fn respond(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);
    let Some(request) = read_request(&mut reader)? else {
        return Ok(());
    };

    let payload = if request.line.starts_with("POST /echo") {
        request.body
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

fn header_value<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let (key, value) = line.split_once(':')?;
    key.trim().eq_ignore_ascii_case(name).then_some(value)
}
