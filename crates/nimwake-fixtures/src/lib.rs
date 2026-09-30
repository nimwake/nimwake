//! `ProcessRuntime` 통합 테스트용 fixture 서버들의 공통 코드.
//!
//! 요구사항: `.docs/phases/phase-1/03-milestones/02-v0.0.2-process-runtime.md`
//!
//! 의존성을 최소로 유지한다. fixture가 복잡해지면 테스트가 무엇을 증명하는지 흐려진다.

use std::io::Write;
use std::net::{TcpListener, TcpStream};

/// argv에서 위치 인자를 읽는다. 없거나 파싱할 수 없으면 사용법을 출력하고 종료한다.
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

/// 최소 HTTP 응답만 돌려주는 accept loop. 정상 종료 경로는 없다.
pub fn serve_forever(listener: &TcpListener) -> ! {
    loop {
        if let Ok((stream, _)) = listener.accept() {
            respond(stream);
        }
    }
}

fn respond(mut stream: TcpStream) {
    let body = "ok";
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
