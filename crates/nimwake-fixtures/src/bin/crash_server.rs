//! 기동 직후 비정상 종료하는 fixture. port를 bind하지 않는다.
//!
//! readiness를 기다리는 도중 process가 죽는 경쟁을 재현한다. (PRD §22, §32)

fn main() -> std::process::ExitCode {
    eprintln!("crash-server: failing on purpose");
    std::process::ExitCode::from(1)
}
