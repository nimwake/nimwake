//! 실제 `nimwaked` 프로세스를 띄우고 다루는 도구.
//!
//! 요구사항: PRD §41 E2E Tests.
//!
//! in-process gateway를 쓰는 `nimwake-fixtures`의 통합 테스트와 다른 점은,
//! 여기서는 **바이너리를 그대로 실행한다**는 것이다. CLI 파싱, tracing 초기화,
//! signal 처리, 종료 순서까지 배포되는 것과 같은 경로를 지난다.

#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub use nimwake_fixtures::testing::{
    free_port, get, is_alive, is_listening, request, wait_for, HttpResponse,
};

/// daemon이 listen 주소를 알려줄 때까지 기다리는 시간.
const STARTUP_DEADLINE: Duration = Duration::from_secs(20);

static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

/// workspace 바이너리가 놓이는 디렉터리.
///
/// `CARGO_BIN_EXE_nimwaked`는 `target/<profile>/nimwaked`를 가리킨다.
/// fixture 바이너리도 같은 곳에 있다.
fn bin_dir() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_nimwaked"))
        .parent()
        .expect("the daemon binary has a parent directory")
}

/// fixture 바이너리 경로. 없으면 무엇을 해야 하는지 알려 준다.
pub fn fixture(name: &str) -> PathBuf {
    let path = bin_dir().join(name);
    assert!(
        path.is_file(),
        "fixture `{name}` is missing at {}.\n\
         E2E needs the whole workspace built: `cargo test --workspace`",
        path.display(),
    );
    path
}

/// manifest 하나를 담은 임시 apps 디렉터리.
pub struct Apps {
    root: PathBuf,
}

impl Apps {
    pub fn new() -> Self {
        let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("nimwake-e2e-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp apps directory is created");
        Self { root }
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// `<root>/<name>/nimwake.toml`을 쓴다.
    pub fn manifest(&self, name: &str, body: &str) -> &Self {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir).expect("app directory is created");
        std::fs::write(dir.join("nimwake.toml"), body).expect("manifest is written");
        self
    }

    /// fixture 하나를 실행하는 흔한 형태의 manifest.
    pub fn app(&self, name: &str, port: u16, command: &[String], lifecycle: Lifecycle) -> &Self {
        let command = command
            .iter()
            .map(|part| format!("{part:?}"))
            .collect::<Vec<_>>()
            .join(", ");

        self.manifest(
            name,
            &format!(
                "name = \"{name}\"\n\
                 \n\
                 [server]\n\
                 host = \"{name}.localhost\"\n\
                 port = {port}\n\
                 \n\
                 [runtime]\n\
                 command = [{command}]\n\
                 working_dir = \".\"\n\
                 \n\
                 [lifecycle]\n\
                 idle_timeout_seconds = {}\n\
                 startup_timeout_seconds = {}\n\
                 shutdown_timeout_seconds = {}\n\
                 max_pending_requests = {}\n",
                lifecycle.idle, lifecycle.startup, lifecycle.shutdown, lifecycle.max_pending,
            ),
        )
    }
}

impl Drop for Apps {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Lifecycle {
    pub idle: u64,
    pub startup: u64,
    pub shutdown: u64,
    pub max_pending: usize,
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            idle: 60,
            startup: 10,
            shutdown: 5,
            max_pending: 256,
        }
    }
}

impl Lifecycle {
    pub const fn idle(mut self, seconds: u64) -> Self {
        self.idle = seconds;
        self
    }

    pub const fn startup(mut self, seconds: u64) -> Self {
        self.startup = seconds;
        self
    }

    pub const fn shutdown(mut self, seconds: u64) -> Self {
        self.shutdown = seconds;
        self
    }

    pub const fn max_pending(mut self, limit: usize) -> Self {
        self.max_pending = limit;
        self
    }
}

/// 실행 중인 daemon.
///
/// drop되면 `SIGTERM`을 보내고 종료를 기다린다. 테스트가 실패해도 daemon이 남지 않는다.
pub struct Daemon {
    child: Child,
    port: u16,
    /// daemon이 알린 실제 listen 주소.
    ///
    /// 출력에서 다시 찾지 않고 여기 보관한다. `start`가 돌아온 시점에 그 줄이
    /// 아직 `output`에 들어가지 않았을 수 있고, 실제로 CI에서 그 경쟁 때문에 깨졌다.
    listen: String,
    output: Arc<Mutex<Vec<String>>>,
}

impl Daemon {
    /// `--port 0`으로 띄우고, daemon이 출력한 실제 port를 읽어 온다.
    ///
    /// 고정 port를 쓰면 테스트끼리 충돌한다.
    pub fn start(apps: &Apps) -> Self {
        Self::start_with(apps, &[])
    }

    pub fn start_with(apps: &Apps, extra: &[&str]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nimwaked"));
        command
            .arg("--apps")
            .arg(apps.path())
            .args(["--port", "0"])
            .args(extra)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = command.spawn().expect("nimwaked starts");

        let output = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();

        // stdout과 stderr를 모두 계속 읽는다. 방치하면 pipe가 차면서 daemon이 멈춘다.
        drain(child.stdout.take(), Arc::clone(&output), Some(tx));
        drain(child.stderr.take(), Arc::clone(&output), None);

        let listen = rx.recv_timeout(STARTUP_DEADLINE).unwrap_or_else(|_| {
            panic!(
                "nimwaked never reported a listen address within {STARTUP_DEADLINE:?}\n{}",
                lines(&output).join("\n"),
            )
        });

        let port = listen
            .rsplit(':')
            .next()
            .and_then(|port| port.parse().ok())
            .unwrap_or_else(|| panic!("no port in the reported address `{listen}`"));

        Self {
            child,
            port,
            listen,
            output,
        }
    }

    /// 기동에 실패할 것으로 기대하는 경우. 종료 코드와 출력을 돌려준다.
    pub fn run_expecting_failure(apps: &Apps) -> (i32, String) {
        Self::run(&["--apps", &apps.path().display().to_string(), "--port", "0"])
    }

    /// daemon을 한 번 실행하고 끝날 때까지 기다린다. 종료 코드와 출력을 돌려준다.
    ///
    /// 종료 코드는 supervisor가 재기동 여부를 판단하는 근거다. (PRD §74)
    pub fn run(args: &[&str]) -> (i32, String) {
        let out = Command::new(env!("CARGO_BIN_EXE_nimwaked"))
            .args(args)
            .output()
            .expect("nimwaked runs");

        let code = out.status.code().unwrap_or(-1);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        (code, text)
    }

    /// daemon이 알린 실제 listen 주소.
    pub fn listen_addr(&self) -> &str {
        &self.listen
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn get(&self, host: &str) -> HttpResponse {
        get(self.port, host).unwrap_or_else(|error| panic!("gateway responds: {error}"))
    }

    pub fn try_get(&self, host: &str) -> std::io::Result<HttpResponse> {
        get(self.port, host)
    }

    pub fn post(&self, host: &str, path: &str, body: &str) -> HttpResponse {
        request(self.port, "POST", path, host, Some(body))
            .unwrap_or_else(|error| panic!("gateway responds: {error}"))
    }

    /// daemon이 지금까지 출력한 줄들.
    pub fn output(&self) -> Vec<String> {
        lines(&self.output)
    }

    /// `SIGTERM`을 보내고 종료를 기다린다. 걸린 시간을 돌려준다.
    pub fn stop(&mut self) -> Duration {
        let started = std::time::Instant::now();
        terminate(&mut self.child);
        let _ = self.child.wait();
        started.elapsed()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        terminate(&mut self.child);
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
fn terminate(child: &mut Child) {
    let pid = nix::unistd::Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX));
    // 이미 끝난 프로세스에 보내는 것은 실패가 아니다.
    let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
}

#[cfg(not(unix))]
fn terminate(child: &mut Child) {
    let _ = child.kill();
}

fn drain<R>(reader: Option<R>, sink: Arc<Mutex<Vec<String>>>, listen: Option<mpsc::Sender<String>>)
where
    R: std::io::Read + Send + 'static,
{
    let Some(reader) = reader else { return };

    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let reported = listen.as_ref().and_then(|_| listen_authority(&line));

            // 저장을 먼저 한다. 알린 뒤에 저장하면 기다리던 쪽이 아직 없는 출력을 볼 수 있다.
            if let Ok(mut sink) = sink.lock() {
                sink.push(line);
            }

            if let (Some(sender), Some(authority)) = (listen.as_ref(), reported) {
                let _ = sender.send(authority);
            }
        }
    });
}

/// `listening on http://127.0.0.1:34567 — ...`에서 `127.0.0.1:34567`을 뽑는다.
fn listen_authority(line: &str) -> Option<String> {
    let rest = line.split("listening on http://").nth(1)?;
    Some(rest.split_whitespace().next()?.to_owned())
}

fn lines(sink: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    sink.lock().map(|lines| lines.clone()).unwrap_or_default()
}
