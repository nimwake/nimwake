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

/// 잠든 발자국을 잴 때 쓰는 app 수. (PRD §50)
///
/// 100개에서 app당 비용이 또렷하게 보이고, 세 번 재어 5,596~5,624 KB였다
/// (흔들림 0.5% — 이 모음에서 가장 좁다). 더 늘려도 기울기는 같고 manifest를
/// 쓰는 시간만 늘어난다.
const SLEEPING_APPS: usize = 100;

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

    // **잠든 발자국을 먼저 잰다.** 다른 시나리오가 app을 깨우고 나면 할당이
    // 섞인다. 요청을 보내지 않으므로 빠르다.
    let sleeping = measure_sleeping(&options)?;
    report.sleeping(sleeping);

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
    // **빠진 지표를 말한다.** `직전 대비` 표에서 행이 사라지는 것만으로는
    // "변화 없음"과 "재지 못했음"을 구분할 수 없다. 기기의 proot가 `/proc`
    // 일부를 막으므로 이 경로는 실제로 열린다 — 그런데 개발 기계와 CI에서는
    // 늘 읽혀서 아무도 보지 않았다.
    //
    // baseline을 쓰지 않는 실행에서도 알린다. 지표가 빠진 사실은 baseline을
    // 어디에 쓰는지와 무관하다.
    let measured = report.baseline();
    let missing = report.unmeasured_metrics(&measured);
    if !missing.is_empty() {
        eprintln!(
            "warning: {} of {} tracked metrics are absent — they will not be compared:",
            missing.len(),
            baseline::TRACKED.len(),
        );
        for key in &missing {
            eprintln!("  {key}  (from {})", baseline::metric_source(key));
        }
    }

    if let Some(path) = &options.baseline_out {
        std::fs::write(path, measured.render())
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
    sleeping_apps: usize,
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
            sleeping_apps: SLEEPING_APPS,
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
                "--sleeping-apps" => {
                    options.sleeping_apps = value()?
                        .parse()
                        .map_err(|_| "`--sleeping-apps` expects a number".to_owned())?;
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
/// 잠든 app `N`개와 함께 daemon을 띄우고 자원을 읽는다. (PRD §50)
///
/// 요청을 보내지 않는다 — 전부 `SLEEPING`인 상태를 본다.
fn measure_sleeping(options: &Options) -> Result<SleepingResult, String> {
    // **두 점을 잰다.** 한 점으로는 고정 비용과 app당 비용을 가를 수 없다.
    let one_app_rss = sleeping_rss(options, 1)?.0;
    let (rss, fds) = sleeping_rss(options, options.sleeping_apps)?;

    Ok(SleepingResult {
        apps: options.sleeping_apps,
        rss,
        fds,
        one_app_rss,
    })
}

/// 잠든 app `count`개와 함께 daemon을 띄우고 `(RSS, fd)`를 읽는다.
fn sleeping_rss(options: &Options, count: usize) -> Result<(Option<u64>, Option<usize>), String> {
    let apps = Apps::new(&format!("sleeping{count}")).map_err(|error| error.to_string())?;
    for index in 0..count {
        let port = free_port();
        apps.app(
            &format!("sleeping{index}"),
            port,
            &[options.echo_server.display().to_string(), port.to_string()],
        )
        .map_err(|error| format!("failed to write a manifest: {error}"))?;
    }

    let daemon = Daemon::start(&options.nimwaked, apps.path())?;

    // **기동 직후의 할당이 가라앉을 시간을 준다.** 바로 읽으면 manifest를 읽는
    // 중의 임시 할당이 섞인다.
    std::thread::sleep(Duration::from_secs(2));

    Ok((
        daemon::resident_memory(daemon.pid()),
        daemon::open_descriptors(daemon.pid()),
    ))
}

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

/// 잠든 app이 많을 때 daemon이 쓰는 자원. (PRD §50)
///
/// **scale-to-zero의 정상 상태가 이것이다.** README가 "작은 기계에 앱 여러 개"를
/// 약속하는데, 그 약속의 핵심 수치는 app 하나당 Core가 얼마를 쓰는가다 —
/// 재어 둔 적이 없었다.
///
/// 깨어 있는 app의 메모리는 app의 것이고 Core의 비용이 아니다. 그래서 **아무것도
/// 깨우지 않고** 잰다.
pub struct SleepingResult {
    pub apps: usize,
    pub rss: Option<u64>,
    pub fds: Option<usize>,
    /// app 하나만 있을 때의 RSS. **한계 비용을 내려면 두 점이 필요하다.**
    ///
    /// 총 RSS를 app 수로 나누면 안 된다 — 5 MB쯤의 고정 비용이 섞여 "app당
    /// 56 KB"처럼 보인다. 실제로는 기울기가 5~6 KB다. 처음 그렇게 적었다.
    pub one_app_rss: Option<u64>,
}

impl SleepingResult {
    /// app 하나를 더할 때 늘어나는 RSS(KB). 두 점의 기울기다.
    #[must_use]
    pub fn marginal_kb_per_app(&self) -> Option<f64> {
        let (many, one) = (self.rss?, self.one_app_rss?);
        if self.apps <= 1 {
            return None;
        }
        let added = many.checked_sub(one)? as f64;
        Some(added / (self.apps - 1) as f64 / 1024.0)
    }
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

#[cfg(test)]
mod sleeping_tests {
    use super::SleepingResult;

    fn result(apps: usize, many_kb: u64, one_kb: u64) -> SleepingResult {
        SleepingResult {
            apps,
            rss: Some(many_kb * 1024),
            fds: Some(11),
            one_app_rss: Some(one_kb * 1024),
        }
    }

    /// **총 RSS를 app 수로 나누지 않는다.** 기울기를 낸다. (PRD §50)
    ///
    /// 처음에 총 RSS ÷ app 수로 적었더니 "app당 56 KB"가 나왔다. 5 MB쯤의 고정
    /// 비용이 섞인 값이다 — 실제 한계 비용은 그 열 분의 일이다. 그 차이가
    /// "이 기계에 app 100개를 올릴 수 있나"의 답을 바꾼다.
    #[test]
    fn the_per_app_cost_is_a_slope_not_an_average() {
        // 실제로 잰 값이다. 1개에서 4,960 KB, 100개에서 5,584 KB.
        let measured = result(100, 5_584, 4_960);

        let slope = measured
            .marginal_kb_per_app()
            .expect("두 점이 있으면 기울기가 나온다");

        // (5584 - 4960) / 99 = 6.3 KB
        assert!((slope - 6.3).abs() < 0.1, "{slope}");

        // 평균으로 내면 열 배 가까이 크게 보인다. 그것을 적으면 안 된다.
        let average = 5_584.0 / 100.0;
        assert!(average > slope * 8.0, "평균 {average} vs 기울기 {slope}");
    }

    /// 두 점이 안 되면 기울기를 내지 않는다.
    #[test]
    fn one_point_yields_no_slope() {
        assert_eq!(result(1, 4_960, 4_960).marginal_kb_per_app(), None);

        let missing = SleepingResult {
            apps: 100,
            rss: Some(5_584 * 1024),
            fds: None,
            one_app_rss: None,
        };
        assert_eq!(missing.marginal_kb_per_app(), None);
    }

    /// **RSS가 줄어도 깨지지 않는다.**
    ///
    /// 100개 쪽이 1개 쪽보다 적게 나올 수 있다 — 할당기 때문에 실제로 봤다
    /// (1개 5,056 KB, 10개 5,004 KB). `checked_sub`가 없으면 u64가 되돌아
    /// 감싸서 터무니없는 기울기가 나온다.
    #[test]
    fn a_smaller_many_app_rss_does_not_wrap() {
        let shrunk = result(100, 5_004, 5_056);
        assert_eq!(
            shrunk.marginal_kb_per_app(),
            None,
            "줄어든 경우는 기울기를 내지 않는다",
        );
    }
}
