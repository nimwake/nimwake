//! 응답 body를 천천히 흘려보내는 fixture.
//!
//! header는 즉시 보내고 body는 나눠서 보낸다.
//! idle timeout보다 오래 걸리게 만들면, gateway가 응답이 끝나기 전에 lease를 반납하는지
//! 드러난다. 반납해 버리면 응답 도중에 process가 정지되어 body가 잘린다. (PRD §25–§26)

use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

/// 한 덩어리 크기와 개수. 총 `CHUNKS * DELAY_MS` 만큼 응답이 이어진다.
const CHUNKS: usize = 20;
const CHUNK: &[u8] = b"0123456789";
const DELAY_MS: u64 = 100;

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let listener = nimwake_fixtures::listen(port);
    println!("slow-body-server listening on 127.0.0.1:{port}");

    loop {
        if let Ok((stream, _)) = listener.accept() {
            let _ = respond(stream);
        }
    }
}

fn respond(mut stream: TcpStream) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n",
        CHUNKS * CHUNK.len()
    )?;
    stream.flush()?;

    for _ in 0..CHUNKS {
        stream.write_all(CHUNK)?;
        stream.flush()?;
        std::thread::sleep(Duration::from_millis(DELAY_MS));
    }

    Ok(())
}

/// 테스트가 기대하는 전체 body 길이.
#[allow(dead_code)]
const fn expected_len() -> usize {
    CHUNKS * CHUNK.len()
}
