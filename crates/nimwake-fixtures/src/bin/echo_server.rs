//! 정상 동작 fixture. bind 후 최소 HTTP 응답만 돌려준다.
//!
//! SIGTERM은 기본 동작(즉시 종료)에 맡긴다. graceful 경로의 기준선이다.

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let listener = nimwake_fixtures::listen(port);
    println!("echo-server listening on 127.0.0.1:{port}");
    nimwake_fixtures::serve_forever(&listener);
}
