//! Lifecycle stress와 누수 관찰.
//!
//! 요구사항: PRD §43 Lifecycle Stress Test.
//!
//! Nimwake는 장시간 실행되는 daemon이므로 순간 최대 RPS보다 lifecycle 안정성이 중요하다.
//!
//! ```text
//! sleep → request → start → serve → idle → stop → sleep
//! ```
//!
//! 이것을 반복하고, 끝난 뒤 남은 것이 있는지 본다.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use nimwake_fixtures::testing::{free_port, get, is_listening};

use crate::daemon::{self, Apps, Daemon};

/// 한 cycle에서 app이 잠들기를 기다리는 한계.
const SLEEP_DEADLINE: Duration = Duration::from_secs(20);

/// 관찰 시점의 daemon 자원 사용량.
#[derive(Debug, Clone, Copy, Default)]
pub struct Resources {
    pub rss: Option<u64>,
    pub fds: Option<usize>,
}

impl Resources {
    fn observe(pid: u32) -> Self {
        Self {
            rss: daemon::resident_memory(pid),
            fds: daemon::open_descriptors(pid),
        }
    }
}

pub struct StressResult {
    pub cycles: usize,
    pub apps: usize,
    pub failures: usize,
    /// 기동된 서로 다른 process 수. cycle 수와 같아야 한다.
    pub spawned: usize,
    /// 끝난 뒤에도 살아 있는 backend. **0이어야 한다.**
    pub survivors: Vec<u32>,
    /// daemon 아래에 남은 zombie. **0이어야 한다.**
    pub zombies: usize,
    /// 아무도 쓰지 않는데 열려 있는 port. **0이어야 한다.**
    pub open_ports: Vec<u16>,
    pub before: Resources,
    pub after: Resources,
    pub wall: Duration,
}

impl StressResult {
    /// correctness gate가 보는 것. 성능 수치는 여기 들어오지 않는다. (PRD §44)
    pub fn leaks(&self) -> Vec<String> {
        let mut problems = Vec::new();

        if self.failures > 0 {
            problems.push(format!("{} request(s) failed", self.failures));
        }
        if !self.survivors.is_empty() {
            problems.push(format!("{} orphan process(es)", self.survivors.len()));
        }
        if self.zombies > 0 {
            problems.push(format!("{} zombie process(es)", self.zombies));
        }
        if !self.open_ports.is_empty() {
            problems.push(format!("{} port(s) left open", self.open_ports.len()));
        }
        if self.spawned != self.cycles {
            problems.push(format!(
                "spawn count {} does not match {} cycles",
                self.spawned, self.cycles
            ));
        }

        problems
    }
}

/// `cycles`번의 lifecycle을 `apps`개에 나눠 수행한다.
///
/// 한 cycle은 idle timeout만큼 걸리고 그 하한은 1초다. 직렬로 돌리면 1000 cycle이
/// 20분 가까이 걸린다. 여러 app에 나누면 같은 횟수를 훨씬 짧은 시간에 돈다.
/// 반복 기동·종료의 누수를 보는 것이 목적이므로 나누어도 목적은 그대로다.
pub fn run(
    nimwaked: &Path,
    echo_server: &Path,
    cycles: usize,
    apps: usize,
) -> Result<StressResult, String> {
    let apps_count = apps.max(1);
    let per_app = cycles.div_ceil(apps_count);
    let total = per_app * apps_count;

    let dir = Apps::new("stress").map_err(|error| error.to_string())?;
    let mut ports = Vec::with_capacity(apps_count);

    for index in 0..apps_count {
        let port = free_port();
        ports.push(port);
        dir.app_with_idle(
            &format!("s{index}"),
            port,
            &[echo_server.display().to_string(), port.to_string()],
            1,
        )
        .map_err(|error| error.to_string())?;
    }

    let daemon = Daemon::start(nimwaked, dir.path())?;
    let before = Resources::observe(daemon.pid());
    let gateway = daemon.port();

    let started = Instant::now();
    let workers: Vec<_> = ports
        .iter()
        .enumerate()
        .map(|(index, port)| {
            let (port, host) = (*port, format!("s{index}.localhost"));
            std::thread::spawn(move || cycle(gateway, &host, port, per_app))
        })
        .collect();

    let mut pids = HashSet::new();
    let mut failures = 0usize;
    for worker in workers {
        match worker.join() {
            Ok(outcome) => {
                pids.extend(outcome.pids);
                failures += outcome.failures;
            }
            Err(_) => failures += per_app,
        }
    }
    let wall = started.elapsed();

    let after = Resources::observe(daemon.pid());
    let zombies = zombies_under(daemon.pid());

    // daemon을 정상 종료시킨 뒤에 본다. child의 수명은 daemon의 수명을 넘지 않는다. (PRD §71)
    drop(daemon);
    std::thread::sleep(Duration::from_millis(500));

    let survivors = pids.iter().copied().filter(|pid| alive(*pid)).collect();
    let open_ports = ports
        .into_iter()
        .filter(|port| is_listening(*port))
        .collect();

    Ok(StressResult {
        cycles: total,
        apps: apps_count,
        failures,
        spawned: pids.len(),
        survivors,
        zombies,
        open_ports,
        before,
        after,
        wall,
    })
}

struct CycleOutcome {
    pids: HashSet<u32>,
    failures: usize,
}

/// `sleep → request → start → serve → idle → stop → sleep`을 `count`번.
fn cycle(gateway: u16, host: &str, app_port: u16, count: usize) -> CycleOutcome {
    let mut pids = HashSet::new();
    let mut failures = 0usize;

    for _ in 0..count {
        match get(gateway, host) {
            Ok(response) if response.status == 200 => {
                pids.insert(response.pid());
            }
            _ => {
                failures += 1;
                continue;
            }
        }

        // idle timeout이 app을 재울 때까지 기다린다. 이것으로 한 cycle이 닫힌다.
        let deadline = Instant::now() + SLEEP_DEADLINE;
        while is_listening(app_port) {
            if Instant::now() > deadline {
                failures += 1;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    CycleOutcome { pids, failures }
}

fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        nimwake_fixtures::testing::is_alive(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// `parent` 아래에 남은 zombie 수.
///
/// `/proc/<pid>/stat`의 3번째 필드가 상태, 4번째가 부모 pid다.
/// 파일 이름에 공백이 들어갈 수 있으므로 닫는 괄호 뒤부터 읽는다.
fn zombies_under(parent: u32) -> usize {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };

    entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                return false;
            };
            let Some(rest) = stat.rsplit(')').next() else {
                return false;
            };
            let mut fields = rest.split_whitespace();
            let state = fields.next();
            let ppid = fields.next().and_then(|value| value.parse::<u32>().ok());
            state == Some("Z") && ppid == Some(parent)
        })
        .count()
}

/// 장시간 warm 트래픽. proxy 경로 자체의 누수를 본다. (PRD §43)
///
/// lifecycle stress가 반복 기동·종료를 본다면 이쪽은 기동 없이 지속되는 경로를 본다.
pub struct SustainedResult {
    pub duration: Duration,
    pub requests: usize,
    pub failures: usize,
    pub samples: Vec<Resources>,
}

impl SustainedResult {
    pub fn leaks(&self) -> Vec<String> {
        if self.failures > 0 {
            vec![format!(
                "{} request(s) failed under sustained load",
                self.failures
            )]
        } else {
            Vec::new()
        }
    }

    /// 처음과 마지막 표본의 차이. bounded인지 사람이 판단한다.
    pub fn growth(&self) -> (Option<i64>, Option<i64>) {
        let (Some(first), Some(last)) = (self.samples.first(), self.samples.last()) else {
            return (None, None);
        };

        let rss = first
            .rss
            .zip(last.rss)
            .map(|(a, b)| i64::try_from(b).unwrap_or(0) - i64::try_from(a).unwrap_or(0));
        let fds = first
            .fds
            .zip(last.fds)
            .map(|(a, b)| i64::try_from(b).unwrap_or(0) - i64::try_from(a).unwrap_or(0));

        (rss, fds)
    }
}

pub fn sustained(
    nimwaked: &Path,
    echo_server: &Path,
    duration: Duration,
    connections: usize,
) -> Result<SustainedResult, String> {
    let dir = Apps::new("sustained").map_err(|error| error.to_string())?;
    let port = free_port();
    dir.app(
        "warm",
        port,
        &[echo_server.display().to_string(), port.to_string()],
    )
    .map_err(|error| error.to_string())?;

    let daemon = Daemon::start(nimwaked, dir.path())?;
    let gateway = daemon.port();
    get(gateway, "warm.localhost").map_err(|error| format!("warm-up failed: {error}"))?;

    let deadline = Instant::now() + duration;
    let workers: Vec<_> = (0..connections.max(1))
        .map(|_| {
            std::thread::Builder::new()
                .stack_size(64 * 1024)
                .spawn(move || {
                    let (mut sent, mut failed) = (0usize, 0usize);
                    while Instant::now() < deadline {
                        match get(gateway, "warm.localhost") {
                            Ok(response) if response.status == 200 => sent += 1,
                            _ => failed += 1,
                        }
                    }
                    (sent, failed)
                })
                .expect("load thread spawns")
        })
        .collect();

    // 부하가 도는 동안 자원 추이를 기록한다.
    let mut samples = Vec::new();
    while Instant::now() < deadline {
        samples.push(Resources::observe(daemon.pid()));
        std::thread::sleep(Duration::from_millis(500));
    }
    samples.push(Resources::observe(daemon.pid()));

    let (mut requests, mut failures) = (0usize, 0usize);
    for worker in workers {
        if let Ok((sent, failed)) = worker.join() {
            requests += sent;
            failures += failed;
        }
    }

    Ok(SustainedResult {
        duration,
        requests,
        failures,
        samples,
    })
}
