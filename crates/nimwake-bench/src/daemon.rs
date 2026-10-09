//! 벤치 대상 `nimwaked`를 띄우고, 구간 계측을 회수한다.
//!
//! 요구사항: PRD §72 Latency Instrumentation.
//!
//! benchmark mode의 JSON 출력을 읽어 모은다. daemon 안에 집계를 넣지 않기로 한 결정에 따른 것이다.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 하나의 기동이 지나온 구간. daemon이 내보낸 `nimwake::startup` event에서 읽는다.
#[derive(Debug, Clone, Copy, Default)]
pub struct StartupSpans {
    pub queue: Duration,
    pub spawn: Duration,
    pub readiness: Duration,
    pub total: Duration,
}

pub struct Daemon {
    child: Child,
    port: u16,
    startups: Arc<Mutex<Vec<StartupSpans>>>,
}

impl Daemon {
    /// `--port 0 --trace-json`으로 띄우고 실제 port를 읽어 온다.
    pub fn start(binary: &Path, apps_dir: &Path) -> Result<Self, String> {
        let mut child = Command::new(binary)
            .arg("--apps")
            .arg(apps_dir)
            .args(["--port", "0", "--trace-json"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start {}: {error}", binary.display()))?;

        let startups = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();

        // pipe를 방치하면 daemon이 멈춘다. 두 스트림 모두 계속 읽는다.
        collect(child.stdout.take(), Arc::clone(&startups), Some(tx));
        collect(child.stderr.take(), Arc::clone(&startups), None);

        let port = rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "nimwaked never reported a listen address".to_owned())?;

        Ok(Self {
            child,
            port,
            startups,
        })
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 지금까지 관측한 기동 구간들.
    pub fn startups(&self) -> Vec<StartupSpans> {
        self.startups
            .lock()
            .map(|spans| spans.clone())
            .unwrap_or_default()
    }

    pub fn clear_startups(&self) {
        if let Ok(mut spans) = self.startups.lock() {
            spans.clear();
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let pid =
                nix::unistd::Pid::from_raw(i32::try_from(self.child.id()).unwrap_or(i32::MAX));
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM);
        }
        #[cfg(not(unix))]
        let _ = self.child.kill();

        let _ = self.child.wait();
    }
}

fn collect<R>(
    reader: Option<R>,
    startups: Arc<Mutex<Vec<StartupSpans>>>,
    listen: Option<mpsc::Sender<u16>>,
) where
    R: std::io::Read + Send + 'static,
{
    let Some(reader) = reader else { return };

    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if let Some(sender) = listen.as_ref() {
                if let Some(port) = listen_port(&line) {
                    let _ = sender.send(port);
                }
            }

            if line.contains("nimwake::startup") {
                if let Some(spans) = parse_startup(&line) {
                    if let Ok(mut startups) = startups.lock() {
                        startups.push(spans);
                    }
                }
            }
        }
    });
}

fn listen_port(line: &str) -> Option<u16> {
    let rest = line.split("listening on http://").nth(1)?;
    let authority = rest.split_whitespace().next()?;
    authority.rsplit(':').next()?.parse().ok()
}

/// JSON 한 줄에서 `..._us` 값을 뽑는다.
///
/// serde를 들이지 않는다. 우리가 직접 내보낸 형식이고, 필요한 것은 정수 네 개뿐이다.
fn parse_startup(line: &str) -> Option<StartupSpans> {
    Some(StartupSpans {
        queue: micros(line, "queue_us")?,
        spawn: micros(line, "spawn_us")?,
        readiness: micros(line, "readiness_us")?,
        total: micros(line, "total_us")?,
    })
}

fn micros(line: &str, field: &str) -> Option<Duration> {
    let needle = format!("\"{field}\":");
    let rest = line.split(&needle).nth(1)?;
    // 숫자로 나오는 것이 정상이지만, 따옴표에 싸여 있어도 읽는다.
    let digits: String = rest
        .trim_start()
        .trim_start_matches('"')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some(Duration::from_micros(digits.parse().ok()?))
}

/// daemon의 메모리 사용량. Linux에서만 읽는다. (PRD §42)
pub fn resident_memory(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

/// daemon이 연 file descriptor 수. leak 관찰용이다. (PRD §73)
pub fn open_descriptors(pid: u32) -> Option<usize> {
    Some(std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?.count())
}

/// `/proc/<pid>/stat`의 시간 단위. (`USER_HZ`)
///
/// 커널 `HZ`와 무관하게 Linux에서 100으로 고정돼 있다. 그래서 상수로 둔다.
const USER_HZ: u64 = 100;

/// daemon이 쓴 CPU 시간. user와 system을 합친다. (PRD §50)
///
/// PRD §50은 CI Linux와 Android 실기기를 **같은 지표로** 비교하라고 한다.
/// 비교 항목에 CPU가 있고, 저사양 기기에서 먼저 무너지는 구간을 찾는 것이 목적이다.
///
/// `/proc/<pid>/stat`의 14·15번째 필드(utime·stime)를 읽는다. 앞쪽 필드에는
/// 공백과 괄호를 담을 수 있는 `comm`이 있으므로 마지막 `)` 뒤부터 센다.
pub fn cpu_time(pid: u32) -> Option<Duration> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit(')').next()?;

    // 잘라낸 뒤의 첫 필드는 state다. utime은 그로부터 11칸 뒤다.
    let mut fields = rest.split_whitespace().skip(11);
    let utime: u64 = fields.next()?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;

    Some(Duration::from_micros(
        (utime + stime).saturating_mul(1_000_000 / USER_HZ),
    ))
}

/// 임시 apps 디렉터리 하나.
pub struct Apps {
    root: PathBuf,
}

impl Apps {
    pub fn new(name: &str) -> std::io::Result<Self> {
        let root =
            std::env::temp_dir().join(format!("nimwake-bench-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    /// 벤치 중에는 잠들면 안 되므로 idle timeout을 길게 둔다.
    pub fn app(&self, name: &str, port: u16, command: &[String]) -> std::io::Result<()> {
        self.app_with_idle(name, port, command, 3600)
    }

    /// lifecycle stress는 반대로 짧은 idle timeout이 필요하다.
    /// 한 cycle의 길이가 사실상 이 값이다.
    pub fn app_with_idle(
        &self,
        name: &str,
        port: u16,
        command: &[String],
        idle_seconds: u64,
    ) -> std::io::Result<()> {
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir)?;

        let command = command
            .iter()
            .map(|part| format!("{part:?}"))
            .collect::<Vec<_>>()
            .join(", ");

        std::fs::write(
            dir.join("nimwake.toml"),
            format!(
                "name = \"{name}\"\n\n\
                 [server]\nhost = \"{name}.localhost\"\nport = {port}\n\n\
                 [runtime]\ncommand = [{command}]\nworking_dir = \".\"\n\n\
                 [lifecycle]\n\
                 idle_timeout_seconds = {idle_seconds}\n\
                 startup_timeout_seconds = 30\n\
                 shutdown_timeout_seconds = 5\n\
                 max_pending_requests = 4096\n",
            ),
        )
    }
}

impl Drop for Apps {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
