//! Nimwake performance baseline.
//!
//! 요구사항: PRD §42 Load Tests, §45, §73 Performance Report.
//!
//! **판정하지 않는다.** 어디를 최적화해야 하는지 알아내기 위한 측정만 한다.
//! 총합 수치만 보면 Nimwake를 고쳐야 할지 application을 고쳐야 할지 구분할 수 없으므로
//! 세 가지를 나눠서 잰다.
//!
//! ```text
//! warm path       direct backend와 비교한 proxy overhead
//! cold start      actor queue / spawn / readiness 구간별 분해
//! concurrency     1 → 1000 sweep, knee point 탐색
//! ```

mod daemon;
mod report;
mod stats;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use nimwake_fixtures::testing::{free_port, get, is_listening, wait_for};

use daemon::{Apps, Daemon};
use report::Report;
use stats::Samples;

/// sweep 단계. PRD §42가 정한 값이다.
const SWEEP: [usize; 7] = [1, 10, 50, 100, 250, 500, 1000];

/// sweep 각 단계에서 연결 하나가 보내는 요청 수.
const REQUESTS_PER_CONNECTION: usize = 20;

/// cold-start storm의 동시 요청 수.
const STORM_REQUESTS: usize = 1000;

/// warm path 표본 수.
const WARM_REQUESTS: usize = 2000;

fn main() -> std::process::ExitCode {
    match run() {
        Ok(report) => {
            print!("{report}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("nimwake-bench: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let options = Options::from_args()?;
    let mut report = Report::new(&options.commit);

    let direct = measure_direct(&options)?;
    let warm = measure_warm(&options)?;
    report.warm_path(direct, warm);

    let storm = measure_cold_start_storm(&options)?;
    report.cold_start(storm);

    for concurrency in SWEEP {
        if concurrency > options.max_concurrency {
            break;
        }
        report.sweep_row(measure_sweep(&options, concurrency)?);
    }

    let text = report.render();
    if let Some(path) = &options.out {
        std::fs::write(path, &text)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        eprintln!("report written to {}", path.display());
    }

    Ok(text)
}

struct Options {
    nimwaked: PathBuf,
    echo_server: PathBuf,
    out: Option<PathBuf>,
    commit: String,
    max_concurrency: usize,
}

impl Options {
    fn from_args() -> Result<Self, String> {
        // 기본값은 자기 자신 옆이다. workspace 바이너리는 모두 같은 디렉터리에 놓인다.
        let bin_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .ok_or("cannot locate the binary directory")?;

        let mut options = Self {
            nimwaked: bin_dir.join("nimwaked"),
            echo_server: bin_dir.join("echo-server"),
            out: None,
            commit: std::env::var("GITHUB_SHA").unwrap_or_else(|_| "unknown".to_owned()),
            max_concurrency: usize::MAX,
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("`{arg}` needs a value"));
            match arg.as_str() {
                "--nimwaked" => options.nimwaked = PathBuf::from(value()?),
                "--echo-server" => options.echo_server = PathBuf::from(value()?),
                "--out" => options.out = Some(PathBuf::from(value()?)),
                "--commit" => options.commit = value()?,
                "--max-concurrency" => {
                    options.max_concurrency = value()?
                        .parse()
                        .map_err(|_| "`--max-concurrency` expects a number".to_owned())?;
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }

        for (label, path) in [
            ("nimwaked", &options.nimwaked),
            ("echo-server", &options.echo_server),
        ] {
            if !path.is_file() {
                return Err(format!(
                    "{label} not found at {}. build the workspace first",
                    path.display()
                ));
            }
        }

        Ok(options)
    }
}

/// Nimwake를 거치지 않은 baseline. 같은 runner에서 재야 비교가 성립한다. (PRD §42)
fn measure_direct(options: &Options) -> Result<stats::Summary, String> {
    let port = free_port();
    let mut backend = Backend::start(&options.echo_server, port)?;

    wait_for("the backend to listen", Duration::from_secs(10), || {
        is_listening(port)
    });

    let summary = load(port, "direct.localhost", 8, WARM_REQUESTS / 8);
    backend.stop();
    Ok(summary)
}

/// 이미 RUNNING인 app에 보내는 요청. Nimwake 자체의 overhead만 남는다. (PRD §42)
fn measure_warm(options: &Options) -> Result<WarmResult, String> {
    let apps = Apps::new("warm").map_err(|error| error.to_string())?;
    let app_port = free_port();
    apps.app(
        "warm",
        app_port,
        &[
            options.echo_server.display().to_string(),
            app_port.to_string(),
        ],
    )
    .map_err(|error| error.to_string())?;

    let daemon = Daemon::start(&options.nimwaked, apps.path())?;

    // 측정 전에 깨워 둔다. cold start가 warm 표본에 섞이면 안 된다.
    get(daemon.port(), "warm.localhost").map_err(|error| format!("warm-up failed: {error}"))?;

    let summary = load(daemon.port(), "warm.localhost", 8, WARM_REQUESTS / 8);

    Ok(WarmResult {
        summary,
        rss: daemon::resident_memory(daemon.pid()),
        fds: daemon::open_descriptors(daemon.pid()),
    })
}

pub struct WarmResult {
    pub summary: stats::Summary,
    pub rss: Option<u64>,
    pub fds: Option<usize>,
}

/// SLEEPING 상태에서 동시에 많은 요청을 보낸다.
///
/// Nimwake에서는 최대 RPS보다 이 쪽이 중요하다. 기동이 하나로 합쳐지는가가 핵심이다. (PRD §42)
fn measure_cold_start_storm(options: &Options) -> Result<StormResult, String> {
    let apps = Apps::new("storm").map_err(|error| error.to_string())?;
    let app_port = free_port();
    apps.app(
        "storm",
        app_port,
        &[
            options.echo_server.display().to_string(),
            app_port.to_string(),
        ],
    )
    .map_err(|error| error.to_string())?;

    let daemon = Daemon::start(&options.nimwaked, apps.path())?;
    daemon.clear_startups();

    let gateway = daemon.port();
    let started = Instant::now();
    let workers: Vec<_> = (0..STORM_REQUESTS)
        .map(|_| spawn_client(move || get(gateway, "storm.localhost")))
        .collect();

    let mut pids = HashSet::new();
    let mut samples = Samples::default();
    let mut failures = 0usize;

    for worker in workers {
        match worker.join() {
            Ok(Ok(response)) if response.status == 200 => {
                pids.insert(response.pid());
            }
            Ok(Ok(_)) | Ok(Err(_)) | Err(_) => failures += 1,
        }
    }
    samples.set_wall(started.elapsed());

    // 기동 구간이 기록될 시간을 준다.
    wait_for("the startup trace", Duration::from_secs(5), || {
        !daemon.startups().is_empty()
    });

    Ok(StormResult {
        requests: STORM_REQUESTS,
        failures,
        unique_pids: pids.len(),
        spans: daemon.startups(),
        wall: started.elapsed(),
    })
}

pub struct StormResult {
    pub requests: usize,
    pub failures: usize,
    pub unique_pids: usize,
    pub spans: Vec<daemon::StartupSpans>,
    pub wall: Duration,
}

/// 동시성을 단계적으로 올리며 knee point를 찾는다. (PRD §42)
fn measure_sweep(options: &Options, concurrency: usize) -> Result<SweepRow, String> {
    let apps = Apps::new(&format!("sweep-{concurrency}")).map_err(|error| error.to_string())?;
    let app_port = free_port();
    apps.app(
        "sweep",
        app_port,
        &[
            options.echo_server.display().to_string(),
            app_port.to_string(),
        ],
    )
    .map_err(|error| error.to_string())?;

    let daemon = Daemon::start(&options.nimwaked, apps.path())?;
    get(daemon.port(), "sweep.localhost").map_err(|error| format!("warm-up failed: {error}"))?;

    let summary = load(
        daemon.port(),
        "sweep.localhost",
        concurrency,
        REQUESTS_PER_CONNECTION,
    );

    Ok(SweepRow {
        concurrency,
        summary,
        rss: daemon::resident_memory(daemon.pid()),
    })
}

pub struct SweepRow {
    pub concurrency: usize,
    pub summary: stats::Summary,
    pub rss: Option<u64>,
}

/// `connections`개를 동시에 열어 각각 `per_connection`번 요청한다.
fn load(port: u16, host: &str, connections: usize, per_connection: usize) -> stats::Summary {
    let started = Instant::now();

    let workers: Vec<_> = (0..connections)
        .map(|_| {
            let host = host.to_owned();
            spawn_client(move || {
                let mut samples = Samples::default();
                for _ in 0..per_connection {
                    let at = Instant::now();
                    match get(port, &host) {
                        Ok(response) if response.status == 200 => samples.record(at.elapsed()),
                        _ => samples.record_error(),
                    }
                }
                samples
            })
        })
        .collect();

    let mut samples = Samples::default();
    for worker in workers {
        if let Ok(partial) = worker.join() {
            samples.merge(partial);
        }
    }
    samples.set_wall(started.elapsed());
    samples.summarize()
}

/// 동시성 1000까지 올리므로 기본 스택으로는 주소 공간이 아깝다.
fn spawn_client<T, F>(work: F) -> std::thread::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(work)
        .expect("client thread spawns")
}

/// Nimwake를 거치지 않는 비교용 backend.
struct Backend {
    child: Child,
}

impl Backend {
    fn start(binary: &Path, port: u16) -> Result<Self, String> {
        let child = Command::new(binary)
            .arg(port.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to start the direct backend: {error}"))?;
        Ok(Self { child })
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop();
    }
}
