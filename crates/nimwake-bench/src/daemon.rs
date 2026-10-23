//! 벤치 대상 `nimwaked`를 띄우고, 구간 계측을 회수한다.
//!
//! 요구사항: PRD §72 Latency Instrumentation.
//!
//! benchmark mode의 JSON 출력을 읽어 모은다. daemon 안에 집계를 넣지 않기로 한 결정에 따른 것이다.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// 요청 하나가 지나온 구간. daemon이 내보낸 `nimwake::request` event에서 읽는다.
///
/// [`StartupSpans`]와 **중첩 관계다.** cold start 요청에서는 `acquire` 안에
/// 기동 분해(queue · spawn · readiness)가 들어 있다.
///
/// ```text
/// ttfb
/// ├── routing          gateway 자체 처리
/// ├── acquire          깨우기 — cold start면 여기 안에 StartupSpans가 있다
/// ├── proxy connect    backend와 TCP 연결 수립까지
/// └── backend          app이 응답 header를 만든 시간
/// ```
///
/// §50이 비교 항목으로 "spawn / readiness / **proxy connect**"를 적었고,
/// 그 중 proxy connect는 기동 trace가 아니라 이쪽에 있다.
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestSpans {
    pub routing: Duration,
    pub acquire: Duration,
    pub proxy_connect: Duration,
    pub backend: Duration,
    pub ttfb: Duration,
}

pub struct Daemon {
    child: Child,
    port: u16,
    startups: Arc<Mutex<Vec<StartupSpans>>>,
    requests: Arc<Mutex<Vec<RequestSpans>>>,
    /// 기동 구간 trace로 보이는 줄을 몇 개 봤는가.
    ///
    /// **파싱 실패를 그냥 버리는 것은 의도된 것이다** — 404나 acquire 실패로
    /// 끝난 요청은 구간이 모두 없고, 그런 줄은 구간 비교에 쓸 수 없다.
    ///
    /// 그런데 **줄은 봤는데 하나도 못 읽었다면** 그것은 필드 이름이 어긋난
    /// 것이다. core가 `tracing::debug!`의 식별자로 내보내고 bench가 문자열로
    /// 찾으므로 컴파일러가 둘을 묶어 주지 않는다. 그 경우를 구분하려고 센다.
    startup_lines: Arc<AtomicUsize>,
    request_lines: Arc<AtomicUsize>,
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
        let requests = Arc::new(Mutex::new(Vec::new()));
        let startup_lines = Arc::new(AtomicUsize::new(0));
        let request_lines = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = mpsc::channel();

        // pipe를 방치하면 daemon이 멈춘다. 두 스트림 모두 계속 읽는다.
        collect(
            child.stdout.take(),
            Arc::clone(&startups),
            Arc::clone(&requests),
            Arc::clone(&startup_lines),
            Arc::clone(&request_lines),
            Some(tx),
        );
        collect(
            child.stderr.take(),
            Arc::clone(&startups),
            Arc::clone(&requests),
            Arc::clone(&startup_lines),
            Arc::clone(&request_lines),
            None,
        );

        let port = rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| "nimwaked never reported a listen address".to_owned())?;

        Ok(Self {
            startup_lines,
            request_lines,
            child,
            port,
            startups,
            requests,
        })
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 지금까지 관측한 기동 구간들.
    /// 구간을 하나도 못 읽었는데 trace 줄은 봤는가. (PRD §72)
    ///
    /// **그렇다면 필드 이름이 어긋난 것이다.** core가 `tracing::debug!`의
    /// 식별자(`ttfb_us = …`)로 내보내고 bench가 문자열(`"ttfb_us"`)로 찾는다 —
    /// 컴파일러가 둘을 묶어 주지 않으므로 한쪽만 바뀌면 조용히 지표가 사라진다.
    ///
    /// 파싱 실패 자체는 정상이다(404나 acquire 실패로 끝난 요청은 구간이
    /// 없다). **전부 실패한 것**만 드리프트다.
    /// 어긋난 종류의 이름. 둘 다 멀쩡하면 `None`.
    ///
    /// **종류별로 센다.** 처음에 "둘 다 비었는가"로 판정했더니 `ttfb_us` 하나만
    /// 바꾸는 뮤테이션을 놓쳤다 — 요청 구간은 전부 실패하는데 기동 구간은
    /// 멀쩡해서 조건이 거짓이었다.
    #[must_use]
    pub fn drifted_spans(&self) -> Option<&'static str> {
        if self.startup_lines.load(Ordering::Relaxed) > 0 && self.startups().is_empty() {
            return Some("nimwake::startup");
        }
        if self.request_lines.load(Ordering::Relaxed) > 0 && self.requests().is_empty() {
            return Some("nimwake::request");
        }
        None
    }

    pub fn startups(&self) -> Vec<StartupSpans> {
        self.startups
            .lock()
            .map(|spans| spans.clone())
            .unwrap_or_default()
    }

    /// 지금까지 관측한 요청 구간들. (PRD §50)
    pub fn requests(&self) -> Vec<RequestSpans> {
        self.requests
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
    requests: Arc<Mutex<Vec<RequestSpans>>>,
    startup_lines: Arc<AtomicUsize>,
    request_lines: Arc<AtomicUsize>,
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
                startup_lines.fetch_add(1, Ordering::Relaxed);
                if let Some(spans) = parse_startup(&line) {
                    if let Ok(mut startups) = startups.lock() {
                        startups.push(spans);
                    }
                }
            }

            if line.contains("nimwake::request") {
                request_lines.fetch_add(1, Ordering::Relaxed);
                if let Some(spans) = parse_request(&line) {
                    if let Ok(mut requests) = requests.lock() {
                        requests.push(spans);
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

/// JSON 한 줄에서 `..._ns` 값을 뽑는다.
///
/// serde를 들이지 않는다. 우리가 직접 내보낸 형식이고, 필요한 것은 정수 네 개뿐이다.
/// `nimwake::request` 한 줄에서 구간을 뽑는다.
///
/// 모든 필드가 있어야 한다. 하나라도 없으면 그 요청은 끝까지 가지 않았다는 뜻이고
/// (예: 404, acquire 실패) 구간 비교에 쓸 수 없다.
fn parse_request(line: &str) -> Option<RequestSpans> {
    Some(RequestSpans {
        routing: nanos(line, "routing_ns")?,
        acquire: nanos(line, "acquire_ns")?,
        proxy_connect: nanos(line, "proxy_connect_ns")?,
        backend: nanos(line, "backend_ns")?,
        ttfb: nanos(line, "ttfb_ns")?,
    })
}

fn parse_startup(line: &str) -> Option<StartupSpans> {
    Some(StartupSpans {
        queue: nanos(line, "queue_ns")?,
        spawn: nanos(line, "spawn_ns")?,
        readiness: nanos(line, "readiness_ns")?,
        total: nanos(line, "total_ns")?,
    })
}

fn nanos(line: &str, field: &str) -> Option<Duration> {
    let needle = format!("\"{field}\":");
    let rest = line.split(&needle).nth(1)?;
    // 숫자로 나오는 것이 정상이지만, 따옴표에 싸여 있어도 읽는다.
    let digits: String = rest
        .trim_start()
        .trim_start_matches('"')
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    Some(Duration::from_nanos(digits.parse().ok()?))
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
