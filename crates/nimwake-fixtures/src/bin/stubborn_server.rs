//! SIGTERM을 무시하는 fixture.
//!
//! graceful 종료가 실패했을 때 shutdown timeout 뒤 SIGKILL이 수행되는지 검증한다. (PRD §30)

#[cfg(unix)]
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};

#[cfg(unix)]
fn main() {
    let port: u16 = nimwake_fixtures::arg(1, "port");

    let ignore = SigAction::new(SigHandler::SigIgn, SaFlags::empty(), SigSet::empty());
    // sigaction은 프로세스 전역 상태를 바꾸므로 nix에서도 unsafe다.
    // 이 fixture의 존재 이유 자체가 SIGTERM 무시이므로 여기서만 허용한다.
    #[allow(unsafe_code)]
    unsafe {
        sigaction(Signal::SIGTERM, &ignore).expect("failed to ignore SIGTERM");
    }

    let listener = nimwake_fixtures::listen(port);
    println!("stubborn-server listening on 127.0.0.1:{port}, ignoring SIGTERM");
    nimwake_fixtures::serve_forever(&listener);
}

#[cfg(not(unix))]
fn main() {
    panic!("stubborn-server exercises POSIX signals and only runs on Unix");
}
