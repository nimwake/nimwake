//! stdout을 끊임없이 쏟아 내는 fixture.
//!
//! pipe를 만들어 두고 읽지 않으면 buffer가 차는 순간 child가 멈춘다.
//! daemon이 drain하고 있다면 이 fixture는 계속 응답할 수 있어야 한다. (PRD §19)

use std::io::Write;

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let listener = nimwake_fixtures::listen(port);

    std::thread::spawn(|| {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        let line = "x".repeat(512);
        let mut n: u64 = 0;
        loop {
            n += 1;
            if writeln!(out, "{n} {line}").is_err() {
                return;
            }
        }
    });

    nimwake_fixtures::serve_forever(&listener);
}
