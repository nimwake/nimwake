//! readiness 이후에 죽는 fixture.
//!
//! RUNNING 중 process가 사라지는 상황을 재현한다. (PRD §32)

use std::time::Duration;

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let lifetime_ms: u64 = nimwake_fixtures::arg(2, "lifetime_ms");

    let listener = nimwake_fixtures::listen(port);
    println!("exit-after-server listening on 127.0.0.1:{port} for {lifetime_ms}ms");

    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(lifetime_ms));
        std::process::exit(17);
    });

    nimwake_fixtures::serve_forever(&listener);
}
