//! 받은 header를 그대로 돌려주는 fixture.
//!
//! gateway가 넣은 것을 application이 **실제로 받는지** 확인한다. (PRD §81)
//!
//! `echo-server`는 자기 pid만 돌려주므로 header 전달을 볼 수 없다. 그 차이 때문에
//! `to_backend_request`가 `append_forwarded_for`를 부르지 않게 바꿔도 단위 테스트
//! 일곱 개가 모두 통과했다 — 함수를 직접 부르고 있었기 때문이다.

use std::io::{BufReader, Write};
use std::net::TcpStream;

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let listener = nimwake_fixtures::listen(port);
    println!("header-echo-server listening on 127.0.0.1:{port}");

    loop {
        if let Ok((stream, _)) = listener.accept() {
            // 한 연결의 실패가 서버를 멈추게 하지 않는다.
            let _ = respond(stream);
        }
    }
}

fn respond(stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream);

    // readiness probe는 아무것도 보내지 않고 닫는다. 읽지 않고 쓰면 그 죽은 연결을
    // 붙들고 있는 동안 진짜 요청이 backlog에서 기다린다.
    let Some(request) = nimwake_fixtures::read_request(&mut reader)? else {
        return Ok(());
    };

    let body = request.headers.join("\n");
    let mut stream = reader.into_inner();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len(),
    )?;
    stream.flush()
}
