//! 요청을 처리하는 **그 경로에서** stdout을 쏟아 내는 fixture.
//!
//! pipe를 만들어 두고 읽지 않으면 buffer가 차는 순간 write가 막힌다.
//! daemon이 drain하고 있다면 이 fixture는 계속 응답할 수 있어야 한다. (PRD §19)
//!
//! 처음에는 별도 스레드에서 썼다. 그러면 pipe가 차도 **그 스레드만** 멈추고 main은
//! 계속 응답하므로, drain을 제거해도 테스트가 통과했다. fixture가 이름대로의 위험을
//! 모델링하지 않았던 것이다.
//!
//! 지금은 응답 직전에 쓴다. 로그를 남기고 응답하는 보통의 application과 같은 모양이고,
//! pipe가 막히면 응답도 막힌다.

use std::io::{BufReader, Write};

/// 요청 하나당 쏟아 낼 양. 기본 pipe buffer(64KiB)를 한 번에 넘긴다.
const BURST_BYTES: usize = 128 * 1024;

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let listener = nimwake_fixtures::listen(port);
    println!("noisy-server listening on 127.0.0.1:{port}");

    let line = "x".repeat(511);
    let lines = BURST_BYTES / 512;
    let mut served: u64 = 0;

    loop {
        let Ok((stream, _)) = listener.accept() else {
            continue;
        };

        let mut reader = BufReader::new(stream);
        // readiness probe는 아무것도 보내지 않고 끊는다. 그때는 쏟아 내지 않는다.
        let Ok(Some(_request)) = nimwake_fixtures::read_request(&mut reader) else {
            continue;
        };

        served += 1;

        // 응답하기 전에 쓴다. drain하지 않으면 여기서 막히고 응답이 나가지 못한다.
        {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            for n in 0..lines {
                if writeln!(out, "{served} {n} {line}").is_err() {
                    break;
                }
            }
            let _ = out.flush();
        }

        let payload = std::process::id().to_string();
        let mut stream = reader.into_inner();
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(payload.as_bytes());
        let _ = stream.flush();
    }
}
