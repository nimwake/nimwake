//! Process group 신호 전달. (PRD §71, Invariant 10)
//!
//! child는 전용 process group의 leader로 기동한다(`process_group(0)`).
//! 따라서 group id는 child의 pid와 같고, 신호는 child가 만든 손자 process까지 함께 받는다.
//!
//! `nix`의 안전한 wrapper만 사용한다. workspace가 `unsafe_code = "forbid"`이므로
//! `libc::killpg`를 직접 호출할 수 없고, 그렇게 유지하는 편이 낫다.

use nix::errno::Errno;
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;

/// process group 전체에 `SIGTERM`을 보낸다.
pub(super) fn terminate_group(pid: u32) -> Result<(), Errno> {
    signal_group(pid, Signal::SIGTERM)
}

/// process group 전체에 `SIGKILL`을 보낸다.
pub(super) fn kill_group(pid: u32) -> Result<(), Errno> {
    signal_group(pid, Signal::SIGKILL)
}

fn signal_group(pid: u32, signal: Signal) -> Result<(), Errno> {
    // pid는 spawn 직후 읽은 값이라 i32 범위를 벗어나지 않는다.
    let pgid = Pid::from_raw(i32::try_from(pid).unwrap_or(i32::MAX));

    match killpg(pgid, signal) {
        // group이 이미 사라졌다. 우리가 원하던 상태이므로 실패가 아니다.
        Err(Errno::ESRCH) => Ok(()),
        other => other,
    }
}
