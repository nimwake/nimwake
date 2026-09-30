//! readiness 지연 fixture. bind까지 `delay_ms`를 기다린다.
//!
//! startup timeout보다 긴 지연을 주면 startup timeout 경로를 재현한다. (PRD §22)

use std::time::Duration;

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let delay_ms: u64 = nimwake_fixtures::arg(2, "delay_ms");

    std::thread::sleep(Duration::from_millis(delay_ms));

    let listener = nimwake_fixtures::listen(port);
    println!("slow-start-server listening on 127.0.0.1:{port} after {delay_ms}ms");
    nimwake_fixtures::serve_forever(&listener);
}
