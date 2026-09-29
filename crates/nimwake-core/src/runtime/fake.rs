//! 테스트용 runtime.
//!
//! OS process 없이 lifecycle state machine을 검증하기 위한 구현이다.
//! 요구사항: PRD §18(테스트용 구현), §39–§40.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;

use crate::app::AppSpec;
use crate::error::RuntimeError;
use crate::id::{ProcessId, ProcessIdSeq};

use super::handle::{Backend, ExitStatus, ProcessHandle, StartedProcess};
use super::Runtime;

/// [`FakeRuntime`]의 동작을 조정한다.
#[derive(Debug, Clone, Default)]
pub struct FakeRuntimeConfig {
    /// `start`가 성공을 돌려주기까지 걸리는 시간. readiness 지연을 흉내 낸다.
    pub start_delay: Duration,
    /// `stop`이 완료되기까지 걸리는 시간.
    pub stop_delay: Duration,
    /// 앞으로 `n`번의 `start`를 실패시킨다.
    pub failing_starts: usize,
}

#[derive(Debug)]
struct FakeProcess {
    exit: watch::Sender<Option<ExitStatus>>,
}

#[derive(Debug)]
struct State {
    config: FakeRuntimeConfig,
    processes: HashMap<ProcessId, FakeProcess>,
}

/// 결정적으로 조작 가능한 in-memory runtime.
#[derive(Debug)]
pub struct FakeRuntime {
    state: Mutex<State>,
    process_ids: ProcessIdSeq,
    start_count: AtomicUsize,
    stop_count: AtomicUsize,
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self::new(FakeRuntimeConfig::default())
    }
}

impl FakeRuntime {
    #[must_use]
    pub fn new(config: FakeRuntimeConfig) -> Self {
        Self {
            state: Mutex::new(State {
                config,
                processes: HashMap::new(),
            }),
            process_ids: ProcessIdSeq::new(),
            start_count: AtomicUsize::new(0),
            stop_count: AtomicUsize::new(0),
        }
    }

    /// `Runtime::start`가 호출된 횟수.
    ///
    /// cold-start deduplication의 핵심 검증 지표다. (PRD §40)
    #[must_use]
    pub fn start_count(&self) -> usize {
        self.start_count.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn stop_count(&self) -> usize {
        self.stop_count.load(Ordering::SeqCst)
    }

    /// 아직 종료되지 않은 process 수. Invariant 1 검증에 사용한다.
    #[must_use]
    pub fn alive_processes(&self) -> usize {
        let state = self.lock();
        state
            .processes
            .values()
            .filter(|process| process.exit.borrow().is_none())
            .count()
    }

    pub fn set_start_delay(&self, delay: Duration) {
        self.lock().config.start_delay = delay;
    }

    pub fn set_stop_delay(&self, delay: Duration) {
        self.lock().config.stop_delay = delay;
    }

    /// 다음 `n`번의 `start`를 실패시킨다.
    pub fn fail_next_starts(&self, n: usize) {
        self.lock().config.failing_starts = n;
    }

    /// process가 예기치 않게 죽은 상황을 만든다. (PRD §32)
    ///
    /// 해당 process를 `wait`하던 monitor task가 깨어나 `ProcessExited`를 보고한다.
    pub fn crash(&self, process_id: ProcessId) {
        self.terminate(process_id, ExitStatus::from_code(1));
    }

    fn terminate(&self, process_id: ProcessId, status: ExitStatus) {
        let state = self.lock();
        if let Some(process) = state.processes.get(&process_id) {
            if process.exit.borrow().is_none() {
                // `send`는 수신자가 하나도 없으면 실패한다.
                // monitor task가 아직 subscribe하기 전일 수 있으므로 반드시 값을 남겨야 한다.
                process.exit.send_replace(Some(status));
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl Runtime for FakeRuntime {
    async fn start(&self, spec: &AppSpec) -> Result<StartedProcess, RuntimeError> {
        self.start_count.fetch_add(1, Ordering::SeqCst);

        let (delay, should_fail) = {
            let mut state = self.lock();
            let should_fail = state.config.failing_starts > 0;
            if should_fail {
                state.config.failing_starts -= 1;
            }
            (state.config.start_delay, should_fail)
        };

        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        if should_fail {
            return Err(RuntimeError::new(format!(
                "fake runtime was configured to fail starting `{}`",
                spec.name
            )));
        }

        let process_id = self.process_ids.next_id();
        let (exit, _) = watch::channel(None);
        self.lock()
            .processes
            .insert(process_id, FakeProcess { exit });

        let addr = SocketAddr::from(([127, 0, 0, 1], spec.server.port));
        Ok(StartedProcess {
            handle: ProcessHandle::new(process_id),
            backend: Backend::new(addr),
        })
    }

    async fn wait(&self, handle: &ProcessHandle) -> ExitStatus {
        let receiver = {
            let state = self.lock();
            state
                .processes
                .get(&handle.id())
                .map(|process| process.exit.subscribe())
        };

        let Some(mut receiver) = receiver else {
            return ExitStatus::unknown();
        };

        loop {
            let current = *receiver.borrow_and_update();
            if let Some(status) = current {
                return status;
            }
            if receiver.changed().await.is_err() {
                return ExitStatus::unknown();
            }
        }
    }

    async fn stop(&self, handle: &ProcessHandle, _timeout: Duration) -> Result<(), RuntimeError> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);

        let delay = self.lock().config.stop_delay;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }

        self.terminate(handle.id(), ExitStatus::from_code(0));
        Ok(())
    }
}
