//! bind는 하지만 응답하지 않는 fixture.
//!
//! readiness probe는 통과하므로 application은 RUNNING이 된다.
//! 그 뒤 proxy가 실패하는 경로(502)와, 그때도 lease가 회수되는지를 검증한다. (PRD §26)

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let listener = nimwake_fixtures::listen(port);
    println!("deaf-server listening on 127.0.0.1:{port}, closing every connection");

    loop {
        // accept하자마자 끊는다. client는 응답을 받지 못한다.
        if let Ok((stream, _)) = listener.accept() {
            drop(stream);
        }
    }
}
