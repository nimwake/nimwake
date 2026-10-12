//! `tokio::process::Child`의 유일한 소유자. (PRD §70)
//!
//! Child는 이 파일의 controller task 밖으로 나가지 않는다.
//! 외부는 [`ControllerHandle`]을 통해서만 `wait` / `stop`을 요청한다.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::{debug, info, warn};

use crate::app::AppSpec;
use crate::error::RuntimeError;
use crate::id::ProcessId;
use crate::runtime::handle::{ExitStatus, ProcessControl};

use super::signal::{kill_group, terminate_group};

/// controller mailbox 용량. 동시에 오는 제어 요청은 stop 정도이므로 작아도 된다.
/// 상한이 있다는 것 자체가 Invariant 9의 요구다.
const CONTROL_MAILBOX_CAPACITY: usize = 8;

#[derive(Debug)]
enum Control {
    Stop {
        timeout: Duration,
        ack: oneshot::Sender<Result<(), RuntimeError>>,
    },
}

/// controller task로 가는 제어 경로. `ProcessHandle`이 이것을 들고 다닌다.
#[derive(Debug)]
pub(super) struct ControllerHandle {
    control: mpsc::Sender<Control>,
    exit: watch::Receiver<Option<ExitStatus>>,
    /// OS의 pid. 운영자가 `ps`·`/proc`과 맞춰 보는 값이다. (PRD §79)
    os_pid: u32,
}

impl ControllerHandle {
    pub(super) const fn os_pid(&self) -> u32 {
        self.os_pid
    }
}

#[async_trait]
impl ProcessControl for ControllerHandle {
    async fn wait(&self) -> ExitStatus {
        let mut exit = self.exit.clone();
        loop {
            if let Some(status) = *exit.borrow_and_update() {
                return status;
            }
            if exit.changed().await.is_err() {
                // controller task가 값을 남기지 않고 사라졌다. 관측할 수 있는 것이 없다.
                return ExitStatus::unknown();
            }
        }
    }

    async fn stop(&self, timeout: Duration) -> Result<(), RuntimeError> {
        let (ack, ack_rx) = oneshot::channel();

        if self
            .control
            .send(Control::Stop { timeout, ack })
            .await
            .is_err()
        {
            // controller가 이미 끝났다 == process가 이미 죽었다. stop의 목적은 달성된 상태다.
            return Ok(());
        }

        ack_rx
            .await
            .unwrap_or_else(|_| Err(RuntimeError::new("process controller stopped responding")))
    }
}

/// child를 spawn하고 controller task를 띄운다.
///
/// 이 함수가 반환된 시점에 child는 살아 있지만 **readiness는 아직 확인되지 않았다.** (PRD §22)
pub(super) fn spawn(
    spec: &AppSpec,
    process_id: ProcessId,
) -> Result<Arc<ControllerHandle>, RuntimeError> {
    let mut command = Command::new(&spec.runtime.command[0]);
    command
        .args(&spec.runtime.command[1..])
        .current_dir(&spec.runtime.working_dir)
        .envs(&spec.runtime.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // 전용 process group의 leader로 만든다. 종료 신호가 손자까지 닿는 근거다. (PRD §71)
        .process_group(0)
        // daemon이 비정상 종료해도 child가 남지 않게 한다. (Invariant 10)
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(|error| {
        RuntimeError::new(format!(
            "failed to spawn `{}`: {error}",
            spec.runtime.command.join(" ")
        ))
    })?;

    let Some(pid) = child.id() else {
        return Err(RuntimeError::new(
            "spawned process exited before its pid could be read",
        ));
    };

    // pipe를 만들어 두고 읽지 않으면 buffer가 차는 순간 child가 멈춘다. (PRD §19)
    drain(spec.name.clone(), process_id, &mut child);

    let (exit_tx, exit_rx) = watch::channel(None);
    let (control_tx, control_rx) = mpsc::channel(CONTROL_MAILBOX_CAPACITY);

    tokio::spawn(
        Controller {
            app: spec.name.clone(),
            process_id,
            pid,
            child,
            exit: exit_tx,
        }
        .run(control_rx),
    );

    Ok(Arc::new(ControllerHandle {
        control: control_tx,
        exit: exit_rx,
        os_pid: pid,
    }))
}

struct Controller {
    app: String,
    process_id: ProcessId,
    pid: u32,
    child: Child,
    exit: watch::Sender<Option<ExitStatus>>,
}

impl Controller {
    async fn run(mut self, mut control: mpsc::Receiver<Control>) {
        tokio::select! {
            // child가 스스로 끝났다. crash일 수도, 정상 종료일 수도 있다. (PRD §32)
            result = self.child.wait() => {
                let status = to_exit_status(result);
                info!(
                    app = %self.app,
                    process = %self.process_id,
                    pid = self.pid,
                    %status,
                    "process exited",
                );
                // 직접 child는 거뒀지만 손자가 남아 있을 수 있다.
                self.sweep_group();
                self.finish(status);
            }

            message = control.recv() => match message {
                Some(Control::Stop { timeout, ack }) => {
                    let result = self.shutdown(timeout).await;
                    let _ = ack.send(result);
                }
                // 모든 ProcessHandle이 drop됐다. 이 process를 살려 둘 이유가 남아 있지 않다.
                //
                // startup timeout으로 `Runtime::start`가 취소되는 경로가 여기로 들어온다.
                // 이 정리가 없으면 취소된 cold start마다 child가 고아로 남는다. (Invariant 10)
                None => {
                    warn!(
                        app = %self.app,
                        process = %self.process_id,
                        pid = self.pid,
                        "process handle dropped before stop, terminating",
                    );
                    let _ = self.shutdown(Duration::ZERO).await;
                }
            },
        }
    }

    /// SIGTERM → shutdown timeout → SIGKILL. (PRD §30)
    async fn shutdown(&mut self, timeout: Duration) -> Result<(), RuntimeError> {
        debug!(
            app = %self.app,
            process = %self.process_id,
            pid = self.pid,
            ?timeout,
            "sending SIGTERM to process group",
        );

        if let Err(errno) = terminate_group(self.pid) {
            warn!(app = %self.app, pid = self.pid, %errno, "SIGTERM failed");
        }

        if !timeout.is_zero() {
            if let Ok(result) = tokio::time::timeout(timeout, self.child.wait()).await {
                let status = to_exit_status(result);
                self.sweep_group();
                self.finish(status);
                return Ok(());
            }

            warn!(
                app = %self.app,
                process = %self.process_id,
                pid = self.pid,
                ?timeout,
                "graceful shutdown timed out, sending SIGKILL",
            );
        }

        // SIGKILL은 반드시 reap보다 먼저다. child가 zombie로 남아 있는 동안에만
        // 그 pid(= pgid)가 예약되어 있고, 엉뚱한 group을 죽일 위험이 없다.
        self.sweep_group();

        let status = to_exit_status(self.child.wait().await);
        self.finish(status);
        Ok(())
    }

    fn sweep_group(&self) {
        if let Err(errno) = kill_group(self.pid) {
            warn!(app = %self.app, pid = self.pid, %errno, "SIGKILL to process group failed");
        }
    }

    /// 최종 상태를 공표한다. `wait()`를 기다리던 모든 쪽이 여기서 깨어난다.
    ///
    /// 수신자가 없어도 값은 남아야 하므로 `send_replace`를 쓴다.
    fn finish(&self, status: ExitStatus) {
        self.exit.send_replace(Some(status));
    }
}

fn to_exit_status(result: std::io::Result<std::process::ExitStatus>) -> ExitStatus {
    match result {
        Ok(status) => status
            .code()
            .map_or_else(ExitStatus::unknown, ExitStatus::from_code),
        Err(_) => ExitStatus::unknown(),
    }
}

/// stdout/stderr를 계속 읽어 tracing으로 흘린다. (PRD §19 — pipe 방치 금지)
fn drain(app: String, process_id: ProcessId, child: &mut Child) {
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(forward(app.clone(), process_id, "stdout", stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(forward(app, process_id, "stderr", stderr));
    }
}

async fn forward<R>(app: String, process_id: ProcessId, stream: &'static str, reader: R)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    // `next_line`이 실패하면 pipe가 닫혔거나 유효하지 않은 것이다. 재시도할 가치가 없다.
    while let Ok(Some(line)) = lines.next_line().await {
        info!(app = %app, process = %process_id, stream, "{line}");
    }
}
