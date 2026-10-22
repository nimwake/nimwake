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

mod baseline;
mod daemon;
mod report;
mod stats;
mod stress;

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

/// warm path의 기본 동시 연결 수.
///
/// CI runner의 무릎은 50이므로 8은 그 아래다. 저사양 기기에서는 `--warm-connections`로
/// 내려야 한다. (PRD §50)
const WARM_CONNECTIONS: usize = 8;

/// lifecycle stress 기본값. (PRD §43)
const STRESS_CYCLES: usize = 1000;
const STRESS_APPS: usize = 10;

/// sustained warm load 기본값.
const SUSTAINED_SECONDS: u64 = 30;
const SUSTAINED_CONNECTIONS: usize = 8;

/// crash-loop 기본값. (PRD §33)
///
/// application마다 동시에 기동 하나라는 것을 보려면 여러 개가 필요하다.
const CRASH_LOOP_SECONDS: u64 = 15;
const CRASH_LOOP_APPS: usize = 5;
const CRASH_LOOP_CONNECTIONS: usize = 8;

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

    let warm = measure_warm(&options)?;
    report.warm_path(warm);

    let storm = measure_cold_start_storm(&options)?;
    report.cold_start(storm);

    for concurrency in SWEEP {
        if concurrency > options.max_concurrency {
            break;
        }
        report.sweep_row(measure_sweep(&options, concurrency)?);
    }

    // stress는 오래 걸린다. release gate에서만 켠다. (PRD §43)
    if options.stress {
        eprintln!("running {} lifecycle cycles...", options.stress_cycles);
        report.stress(stress::run(
            &options.nimwaked,
            &options.echo_server,
            options.stress_cycles,
            options.stress_apps,
        )?);

        eprintln!(
            "running sustained warm load for {}s...",
            options.sustained_seconds
        );
        report.sustained(stress::sustained(
            &options.nimwaked,
            &options.echo_server,
            Duration::from_secs(options.sustained_seconds),
            SUSTAINED_CONNECTIONS,
        )?);

        eprintln!("running crash loop for {CRASH_LOOP_SECONDS}s...");
        report.crash_loop(stress::crash_loop(
            &options.nimwaked,
            &options.crash_server,
            Duration::from_secs(CRASH_LOOP_SECONDS),
            CRASH_LOOP_APPS,
            CRASH_LOOP_CONNECTIONS,
        )?);
    }

    // 직전 release와 비교한다. 없으면 이번 것이 첫 baseline이 된다. (PRD §73)
    if let Some(path) = &options.baseline {
        match baseline::Baseline::load(path) {
            Some(previous) => report.compare_with(previous),
            None => eprintln!("no baseline at {}; this run becomes one", path.display()),
        }
    }

    let text = report.render();
    if let Some(path) = &options.out {
        std::fs::write(path, &text)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        eprintln!("report written to {}", path.display());
    }
    if let Some(path) = &options.baseline_out {
        std::fs::write(path, report.baseline().render())
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
        eprintln!("baseline written to {}", path.display());
    }

    // correctness만 실패 사유다. 성능 수치는 기록만 한다. (PRD §44, §73)
    let problems = report.correctness_problems();
    if !problems.is_empty() {
        return Err(format!(
            "correctness gate failed:
  {}",
            problems.join(
                "
  "
            )
        ));
    }

    Ok(text)
}

struct Options {
    nimwaked: PathBuf,
    echo_server: PathBuf,
    crash_server: PathBuf,
    out: Option<PathBuf>,
    /// 직전 release의 baseline. 있으면 비교표를 싣는다.
    baseline: Option<PathBuf>,
    /// 이번 실행을 다음 비교를 위해 남길 곳.
    baseline_out: Option<PathBuf>,
    commit: String,
    max_concurrency: usize,
    /// warm path를 몇 개의 동시 연결로 재는가.
    ///
    /// **무릎보다 높게 재면 뜻이 없다.** 기기의 무릎은 동시성 1이었고 기본값
    /// 8로 재니 direct backend 쪽이 더 느려져 overhead가 음수로 나왔다 —
    /// `echo-server` fixture가 그 농도에서 nimwaked보다 느리기 때문이다.
    /// 비교 기준이 병목이면 overhead를 재는 것이 아니다. (PRD §50)
    warm_connections: usize,
    stress: bool,
    stress_cycles: usize,
    stress_apps: usize,
    sustained_seconds: u64,
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
            crash_server: bin_dir.join("crash-server"),
            out: None,
            baseline: None,
            baseline_out: None,
            commit: std::env::var("GITHUB_SHA").unwrap_or_else(|_| "unknown".to_owned()),
            max_concurrency: usize::MAX,
            warm_connections: WARM_CONNECTIONS,
            stress: false,
            stress_cycles: STRESS_CYCLES,
            stress_apps: STRESS_APPS,
            sustained_seconds: SUSTAINED_SECONDS,
        };

        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("`{arg}` needs a value"));
            match arg.as_str() {
                "--nimwaked" => options.nimwaked = PathBuf::from(value()?),
                "--echo-server" => options.echo_server = PathBuf::from(value()?),
                "--out" => options.out = Some(PathBuf::from(value()?)),
                "--baseline" => options.baseline = Some(PathBuf::from(value()?)),
                "--baseline-out" => options.baseline_out = Some(PathBuf::from(value()?)),
                "--commit" => options.commit = value()?,
                "--stress" => options.stress = true,
                "--stress-cycles" => {
                    options.stress_cycles = value()?
                        .parse()
                        .map_err(|_| "`--stress-cycles` expects a number".to_owned())?;
                }
                "--sustained-seconds" => {
                    options.sustained_seconds = value()?
                        .parse()
                        .map_err(|_| "`--sustained-seconds` expects a number".to_owned())?;
                }
                "--stress-apps" => {
                    options.stress_apps = value()?
                        .parse()
                        .map_err(|_| "`--stress-apps` expects a number".to_owned())?;
                }
                "--warm-connections" => {
                    options.warm_connections = value()?
                        .parse::<usize>()
                        .map_err(|_| "`--warm-connections` expects a number".to_owned())?
                        .max(1);
                }

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

/// 이미 RUNNING인 app으로 가는 요청과 **같은 조건의** direct 요청. (PRD §42, §50)
///
/// 두 경로를 한 번에 잰다. 따로 재면 각자 최대 rps로 밀어 부하가 달라지고,
/// 그 상태로 백분위를 빼면 overhead가 아니다. (`load_paired` 참고)
fn measure_warm(options: &Options) -> Result<WarmResult, String> {
    let direct_port = free_port();
    let mut backend = Backend::start(&options.echo_server, direct_port)?;
    wait_for("the backend to listen", Duration::from_secs(10), || {
        is_listening(direct_port)
    });

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

    // 쌍마다 두 요청을 보내므로 쌍 수는 절반이다. 총 요청 수는 같다.
    let connections = options.warm_connections;
    let paired = load_paired(
        direct_port,
        daemon.port(),
        "warm.localhost",
        connections,
        (WARM_REQUESTS / 2).div_ceil(connections * 2).max(1),
    );

    // **구간을 하나도 못 읽었으면 멈춘다.** (PRD §72)
    //
    // core가 `tracing::debug!`의 식별자로 trace를 내보내고 bench가 문자열로
    // 찾는다. 컴파일러가 둘을 묶어 주지 않으므로 한쪽만 바뀌면 baseline에서
    // `request_*_ms`가 **조용히 사라진다.**
    //
    // 파싱 실패 자체는 정상이다 — 404나 acquire 실패로 끝난 요청은 구간이
    // 없다. 여기서는 warm 요청 수천 개를 성공시킨 뒤이므로, 그래도 하나도
    // 못 읽었다면 이름이 어긋난 것이다.
    if let Some(target) = daemon.drifted_spans() {
        return Err(format!(
            "the daemon emitted `{target}` trace lines but none parsed. The span field names in nimwake-core's trace.rs and this crate's daemon.rs have drifted apart (PRD §72)."
        ));
    }

    let result = WarmResult {
        paired,
        connections,
        rss: daemon::resident_memory(daemon.pid()),
        fds: daemon::open_descriptors(daemon.pid()),
    };
    backend.stop();
    Ok(result)
}
/// direct backend와 gateway를 **번갈아** 때린다. (PRD §50)
///
/// 두 경로를 따로 돌리면 각자 낼 수 있는 최대 rps로 밀기 때문에 기계에 걸리는
/// 부하가 다르다. 그 상태로 백분위를 빼면 overhead가 아니다 — 기기에서는
/// direct 50 rps 대 nimwake 25 rps로 direct 쪽이 더 느려져 **부호까지 뒤집혔다.**
///
/// 번갈아 보내면 두 경로가 같은 worker 수, 같은 순간의 기계 상태를 공유한다.
/// 그리고 **요청 쌍마다 차이를 기록한다** — 백분위를 빼는 것이 아니라
/// 차이의 백분위를 낸다. 전자는 순서통계량 차이이고 후자가 overhead다.
fn load_paired(
    direct_port: u16,
    gateway_port: u16,
    gateway_host: &str,
    connections: usize,
    per_connection: usize,
) -> PairedLoad {
    let started = Instant::now();

    let workers: Vec<_> = (0..connections)
        .map(|_| {
            let gateway_host = gateway_host.to_owned();
            spawn_client(move || {
                let mut direct = Samples::default();
                let mut gateway = Samples::default();
                // 부호 있는 나노초로 담는다. `Duration`은 음수를 담지 못하고,
                // 음수를 0으로 내리면 분포가 위로 치우쳐 overhead를 **크게** 만든다.
                let mut diffs: Vec<i64> = Vec::with_capacity(per_connection);

                for _ in 0..per_connection {
                    // 순서를 고정한다. direct가 늘 먼저이므로 gateway는 조금 뒤의
                    // 기계 상태를 본다. 그 치우침은 overhead를 크게 만드는 쪽이다 —
                    // 작게 만드는 쪽으로 틀리지 않는다.
                    let at = Instant::now();
                    let direct_ok =
                        matches!(get(direct_port, "direct.localhost"), Ok(r) if r.status == 200);
                    let direct_latency = at.elapsed();

                    let at = Instant::now();
                    let gateway_ok =
                        matches!(get(gateway_port, &gateway_host), Ok(r) if r.status == 200);
                    let gateway_latency = at.elapsed();

                    if direct_ok {
                        direct.record(direct_latency);
                    } else {
                        direct.record_error();
                    }

                    if gateway_ok {
                        gateway.record(gateway_latency);
                    } else {
                        gateway.record_error();
                    }

                    // 둘 다 성공한 쌍만 센다. 한쪽이 실패한 쌍의 차이는 뜻이 없다.
                    if direct_ok && gateway_ok {
                        let g = i64::try_from(gateway_latency.as_nanos()).unwrap_or(i64::MAX);
                        let d = i64::try_from(direct_latency.as_nanos()).unwrap_or(i64::MAX);
                        diffs.push(g - d);
                    }
                }

                (direct, gateway, diffs)
            })
        })
        .collect();

    let mut direct = Samples::default();
    let mut gateway = Samples::default();
    let mut diffs: Vec<i64> = Vec::new();

    for worker in workers {
        if let Ok((partial_direct, partial_gateway, partial_diffs)) = worker.join() {
            direct.merge(partial_direct);
            gateway.merge(partial_gateway);
            diffs.extend(partial_diffs);
        }
    }

    let wall = started.elapsed();
    direct.set_wall(wall);
    gateway.set_wall(wall);

    let inverted = diffs.iter().filter(|diff| **diff < 0).count();

    PairedLoad {
        direct: direct.summarize(),
        gateway: gateway.summarize(),
        overhead_p50_ns: signed_percentile(&mut diffs, 0.50),
        overhead_p95_ns: signed_percentile(&mut diffs, 0.95),
        pairs: diffs.len(),
        inverted,
    }
}

/// 부호 있는 나노초의 백분위. 가장 가까운 순위를 쓴다.
///
/// `stats::percentile_of`는 `Duration`을 받으므로 음수를 담을 수 없다.
/// 쌍마다의 차이는 음수가 될 수 있고 **그것을 지우지 않는다.**
fn signed_percentile(values: &mut [i64], percent: f64) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let rank = (percent * (values.len() - 1) as f64).round() as usize;
    values.get(rank.min(values.len() - 1)).copied()
}

/// 짝지어 측정한 warm path. (PRD §50)
pub struct PairedLoad {
    pub direct: stats::Summary,
    pub gateway: stats::Summary,
    /// 요청 쌍마다의 차이에서 낸 백분위. **백분위를 뺀 값이 아니다.**
    pub overhead_p50_ns: Option<i64>,
    pub overhead_p95_ns: Option<i64>,
    /// 둘 다 성공한 쌍의 수.
    pub pairs: usize,
    /// gateway가 더 빨랐던 쌍의 수. 잡음의 크기를 보여 준다.
    pub inverted: usize,
}

pub struct WarmResult {
    pub paired: PairedLoad,
    /// 이 측정에 쓴 동시 연결 수. 무릎과 비교해야 뜻이 있다. (PRD §50)
    pub connections: usize,
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
