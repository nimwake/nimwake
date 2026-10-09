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

/// 지속 부하 중 모을 자원 표본의 목표 개수.
///
/// 길이와 무관하게 비슷한 해상도를 유지하려는 것이다.
const SAMPLE_TARGET: usize = 200;

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

/// daemon의 살아 있는 자식 수. zombie는 세지 않는다.
///
/// crash-loop에서 실제로 묻고 싶은 것은 "동시에 몇 개가 떠 있나"다.
/// 짧게 살고 죽는 process라 표본 간격을 촘촘히 해야 잡힌다.
fn children_under(parent: u32) -> usize {
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
            state != Some("Z") && ppid == Some(parent)
        })
        .count()
}

pub struct CrashLoopResult {
    pub duration: Duration,
    pub apps: usize,
    pub requests: usize,
    /// 502가 아닌 응답. 계속 실패하는 application은 모든 요청에 502여야 한다.
    pub unexpected: usize,
    /// 동시에 살아 있던 자식의 최대값. application 수를 넘으면 안 된다.
    pub max_children: usize,
    /// 부하가 멈추고 거둘 시간을 준 뒤의 zombie 수. **이쪽이 판정 기준이다.**
    pub zombies: usize,
    /// 부하 중에 본 zombie의 최대값. 판정하지 않고 기록만 한다.
    ///
    /// 자식이 종료한 뒤 부모가 `wait`로 거둘 때까지 짧게 zombie다. 촘촘히 보면
    /// 그 창에 걸리므로 0이 아닌 것이 정상이다. 쌓이는지가 문제이고 그것은
    /// [`Self::zombies`]가 답한다.
    pub max_zombies_seen: usize,
    pub samples: Vec<Resources>,
}

impl CrashLoopResult {
    pub fn leaks(&self) -> Vec<String> {
        let mut found = Vec::new();

        if self.unexpected > 0 {
            found.push(format!(
                "{} response(s) were not 502 from a crash-looping app",
                self.unexpected
            ));
        }

        // 기동 중복 제거가 깨지면 여기서 드러난다. application마다 동시에 하나다. (PRD §20)
        if self.max_children > self.apps {
            found.push(format!(
                "{} concurrent child process(es) for {} crash-looping app(s)",
                self.max_children, self.apps
            ));
        }

        // 부하가 멈춘 뒤에도 남아 있으면 거두지 않은 것이다.
        if self.zombies > 0 {
            found.push(format!(
                "{} zombie(s) left unreaped after the crash loop settled",
                self.zombies
            ));
        }

        found
    }

    /// 처음과 마지막 표본의 차이. [`SustainedResult::growth`]와 같은 기준이다.
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

/// 계속 실패하는 application에 지속적으로 요청을 보낸다. (PRD §33)
///
/// PRD §33은 crash-loop backoff를 Post-MVP로 미루면서 이렇게 적는다.
///
/// > MVP에서는 backoff 없이 재시도하되, 반복 실패가 무한 spawn으로 이어지지 않도록
/// > `startup_timeout_seconds`와 bounded waiter 상한에 의존한다.
///
/// 이 시나리오는 그 주장을 확인한다. backoff를 넣지 않는다. 넣는 것은 PRD를
/// 다시 쓰는 일이다. 확인하는 것은 **무엇을 근거로 안전하다고 말하는가**다.
///
/// `crash-server`는 port를 bind하지 않고 곧바로 종료하므로 요청마다 기동이
/// 재시도된다. 자식은 매우 짧게 살기 때문에 표본 간격을 촘촘히 둔다.
pub fn crash_loop(
    nimwaked: &Path,
    crash_server: &Path,
    duration: Duration,
    apps: usize,
    connections: usize,
) -> Result<CrashLoopResult, String> {
    let apps = apps.max(1);
    let dir = Apps::new("crash-loop").map_err(|error| error.to_string())?;

    let hosts: Vec<String> = (0..apps)
        .map(|index| {
            let name = format!("crash{index}");
            let port = free_port();
            dir.app(
                &name,
                port,
                &[crash_server.display().to_string(), port.to_string()],
            )
            .map(|()| format!("{name}.localhost"))
            .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;

    let daemon = Daemon::start(nimwaked, dir.path())?;
    let gateway = daemon.port();
    let deadline = Instant::now() + duration;

    let workers: Vec<_> = (0..connections.max(1))
        .map(|index| {
            let hosts = hosts.clone();
            std::thread::Builder::new()
                .stack_size(64 * 1024)
                .spawn(move || {
                    let (mut sent, mut unexpected) = (0usize, 0usize);
                    let mut next = index;
                    while Instant::now() < deadline {
                        let host = &hosts[next % hosts.len()];
                        next = next.wrapping_add(1);
                        match get(gateway, host) {
                            // 기동에 실패한 application은 502다. (PRD §26)
                            Ok(response) if response.status == 502 => sent += 1,
                            Ok(_) => {
                                sent += 1;
                                unexpected += 1;
                            }
                            // 연결 자체가 실패한 것은 gateway가 답하지 않은 것이다.
                            Err(_) => unexpected += 1,
                        }
                    }
                    (sent, unexpected)
                })
                .expect("load thread spawns")
        })
        .collect();

    // 자식이 짧게 살고 죽으므로 자원 표본보다 촘촘히 본다.
    let mut samples = Vec::new();
    let mut max_children = 0;
    let mut max_zombies_seen = 0;
    let mut next_sample = Instant::now();
    let resource_interval =
        (duration / u32::try_from(SAMPLE_TARGET).unwrap_or(1)).max(Duration::from_millis(500));

    while Instant::now() < deadline {
        max_children = max_children.max(children_under(daemon.pid()));
        max_zombies_seen = max_zombies_seen.max(zombies_under(daemon.pid()));

        if Instant::now() >= next_sample {
            samples.push(Resources::observe(daemon.pid()));
            next_sample = Instant::now() + resource_interval;
        }

        std::thread::sleep(Duration::from_millis(20));
    }
    samples.push(Resources::observe(daemon.pid()));

    let (mut requests, mut unexpected) = (0usize, 0usize);
    for worker in workers {
        let (sent, bad) = worker.join().map_err(|_| "load thread panicked")?;
        requests += sent;
        unexpected += bad;
    }

    // 부하가 멈춘 뒤에 본다. 거둘 시간을 주지 않고 세면 정상적인 일시 zombie를
    // 누수로 읽는다. 실제로 그렇게 한 번 잘못 판정했다.
    std::thread::sleep(Duration::from_millis(500));
    let zombies = zombies_under(daemon.pid());

    Ok(CrashLoopResult {
        duration,
        apps,
        requests,
        unexpected,
        max_children,
        zombies,
        max_zombies_seen,
        samples,
    })
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
    //
    // 간격을 길이에 맞춘다. 500ms로 고정하면 24시간 soak에서 표본이 17만 개가 되고
    // 리포트가 그만큼 길어진다. 추세를 보는 데 필요한 것은 그렇게 많지 않다.
    let interval =
        (duration / u32::try_from(SAMPLE_TARGET).unwrap_or(1)).max(Duration::from_millis(500));

    let mut samples = Vec::new();
    while Instant::now() < deadline {
        samples.push(Resources::observe(daemon.pid()));
        std::thread::sleep(interval);
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
