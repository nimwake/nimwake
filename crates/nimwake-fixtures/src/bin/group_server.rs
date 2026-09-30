//! 자식을 하나 더 만드는 fixture.
//!
//! daemon이 종료 신호를 process group 단위로 보내는지 검증한다.
//! 손자의 pid를 파일로 남겨 테스트가 생존 여부를 관측할 수 있게 한다. (PRD §71)

use std::process::{Command, Stdio};

fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");
    let pid_file: std::path::PathBuf = nimwake_fixtures::arg(2, "pid_file");

    // 손자를 reap하지 않는 것이 이 fixture의 요점이다.
    // daemon이 process group 단위로 정리하지 않으면 이 프로세스가 그대로 남는다.
    #[allow(clippy::zombie_processes)]
    let child = Command::new("sleep")
        .arg("600")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn grandchild");

    std::fs::write(&pid_file, child.id().to_string()).expect("failed to write pid file");

    let listener = nimwake_fixtures::listen(port);
    println!(
        "group-server listening on 127.0.0.1:{port}, grandchild {}",
        child.id()
    );
    nimwake_fixtures::serve_forever(&listener);
}
