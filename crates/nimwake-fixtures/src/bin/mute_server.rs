//! 연결을 받아 두고 응답하지 않는 fixture.
//!
//! `deaf-server`와 다르다. deaf는 accept하자마자 끊으므로 proxy가 실패하고 502가
//! 된다. 이쪽은 연결을 **붙잡는다.** 요청을 읽지도, 응답을 쓰지도, 끊지도 않는다.
//!
//! gateway에는 요청 단위 제한이 없으므로 그 요청은 끝나지 않는다.
//! port는 열려 있어 readiness probe도 startup timeout도 이것을 걸러내지 못한다.

use std::net::TcpStream;

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let listener = nimwake_fixtures::listen(port);
    println!("mute-server listening on 127.0.0.1:{port}, holding every connection");

    // 받은 연결을 들고 있는다. drop하면 끊기므로 client가 오류를 보게 된다.
    let mut held: Vec<TcpStream> = Vec::new();

    loop {
        if let Ok((stream, _)) = listener.accept() {
            held.push(stream);
        }
    }
}
