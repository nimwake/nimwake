//! Nimwake daemon.
//!
//! ```text
//! manifest 읽기 → 기동 단계 검증 → registry 구성 → gateway serve → 종료 시 정리
//! ```
//!
//! 검증 실패를 런타임까지 미루면 cold start 시점에야 port 충돌이 드러난다. (PRD §21)
//!
//! 외부에서 본 동작(인자, 종료 코드, listen 주소, health probe)은 계약이다.
//! supervisor가 여기에 의존한다. (PRD §74 Host Interface)
//!
//! serving은 Unix에서만 가능하다. `ProcessRuntime`이 POSIX 신호 위에 서 있기 때문이다. (PRD §19, §48)
//! 다른 플랫폼에서는 manifest 검증까지만 수행한다.

mod exit;
mod options;

/// eviction이 가용 메모리를 보는 간격. (PRD §80)
///
/// `/proc/meminfo` 판독은 5.7µs이므로 간격 자체의 비용은 없다. 정지가 메모리를
/// 즉시 돌려주지 않으므로 **다음 판단까지 빠져나갈 시간을 주는 쪽**으로 정한다.
const EVICTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

use std::path::Path;
use std::process::ExitCode;

use nimwake_core::registry;
use nimwake_core::AppSpec;
use tracing::error;

use exit::Failure;
use options::{Options, Parsed};

fn main() -> ExitCode {
    let listen_env = std::env::var("NIMWAKE_LISTEN").ok();

    let options = match options::parse(std::env::args().skip(1), listen_env.as_deref()) {
        Ok(Parsed::Options(options)) => *options,
        Ok(Parsed::Help) => {
            print!("{}", options::USAGE);
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Version) => {
            // 한 줄만 낸다. 호스트가 파싱한다. (PRD §74)
            println!("nimwaked {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(failure) => {
            // tracing이 아직 없다. 인자 오류는 사람이 보는 것이므로 stderr로 직접 적는다.
            eprintln!("{failure}\n\n{}", options::USAGE);
            return failure.exit_code();
        }
    };

    init_tracing(options.trace_json);

    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            error!(code = failure.code(), "{failure}");
            failure.exit_code()
        }
    }
}

/// benchmark mode에서는 구간 계측을 JSON 한 줄씩 내보낸다.
///
/// 상시 metrics framework를 붙이지 않는다는 결정에 따른 것이다. 수집과 집계는
/// 이 출력을 읽는 쪽이 한다. (PRD §72)
///
/// 요청 구간(`nimwake::request`)은 요청마다 나가므로 평소에는 꺼 둔다.
/// benchmark mode일 때만 기본 필터를 넓힌다. `RUST_LOG`를 준 쪽이 언제나 우선이다.
fn init_tracing(json: bool) {
    use std::io::IsTerminal;

    let fallback = if json {
        "info,nimwake::request=debug"
    } else {
        "info"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(fallback));

    // 로그는 stdout으로 간다. **터미널이 아니면 색을 칠하지 않는다.**
    //
    // 기본값은 켬이다. supervisor가 파일로 리다이렉트하면 escape sequence가
    // 그대로 파일에 쌓인다. 72h 기기 soak의 로그에서 **39.3%가 escape였다** —
    // 1.19MB 중 492KB. grep과 사람이 읽는 것도 같이 나빠진다.
    //
    // `NO_COLOR`는 tracing-subscriber가 따로 본다. 이 판정은 그것과 별개로
    // "리다이렉트되었는가"를 본다.
    let ansi = std::io::stdout().is_terminal();

    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_current_span(false)
            .with_span_list(false)
            .with_ansi(ansi)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(ansi)
            .init();
    }
}

fn run(options: &Options) -> Result<(), Failure> {
    let specs = load(&options.apps_dir, options.listen.port())?;

    println!(
        "nimwaked {} — {} application(s) validated",
        env!("CARGO_PKG_VERSION"),
        specs.len()
    );

    for spec in &specs {
        println!(
            "  {:<16} {:<28} -> 127.0.0.1:{:<6} idle {}s",
            spec.name, spec.server.host, spec.server.port, spec.lifecycle.idle_timeout_seconds
        );
    }
    println!();

    serve(specs, options)
}

/// manifest 문제는 모두 설정 오류다. 재기동해도 나아지지 않는다. (PRD §74)
fn load(apps_dir: &Path, gateway_port: u16) -> Result<Vec<AppSpec>, Failure> {
    let specs = registry::load_dir(apps_dir).map_err(|error| {
        Failure::config(format!(
            "failed to load manifests from `{}`: {error}",
            apps_dir.display()
        ))
    })?;

    if specs.is_empty() {
        return Err(Failure::config(format!(
            "no manifests found under `{}`",
            apps_dir.display()
        )));
    }

    registry::validate(&specs, gateway_port)
        .map_err(|error| Failure::config(format!("manifest validation failed: {error}")))?;

    Ok(specs)
}

#[cfg(unix)]
fn serve(specs: Vec<AppSpec>, options: &Options) -> Result<(), Failure> {
    use std::sync::Arc;

    use tokio::signal::unix::SignalKind;

    use nimwake_core::runtime::ProcessRuntime;
    use nimwake_core::{AppRegistry, Gateway};
    use tracing::{info, warn};

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Failure::other(format!("failed to start the async runtime: {error}")))?;

    let requested = options.listen;
    let apps_dir = options.apps_dir.clone();
    let limits = options.limits;

    runtime.block_on(async move {
        let runtime: Arc<dyn nimwake_core::Runtime> = Arc::new(ProcessRuntime::new());
        let registry = AppRegistry::build(specs, requested.port(), Arc::clone(&runtime), limits)
            .map_err(|error| Failure::config(format!("registry validation failed: {error}")))?;
        let registry = Arc::new(registry);

        // bind 실패는 시간이 해결할 수 있다. 이전 인스턴스가 아직 port를 쥐고 있을 수 있다.
        let gateway = Gateway::bind(requested, Arc::clone(&registry), options.request_limits)
            .await
            .map_err(|error| {
                Failure::unavailable(format!("failed to bind {requested}: {error}"))
            })?;

        // `--port 0`으로 띄웠다면 실제 배정된 값을 알려야 한다. 테스트와 스크립트가 이 줄을 읽는다.
        let addr = gateway.local_addr().map_err(|error| {
            Failure::other(format!("failed to read the listen address: {error}"))
        })?;

        // **신호를 알림보다 먼저 등록한다.**
        //
        // `SIGHUP`과 `SIGUSR1`의 기본 동작은 **프로세스 종료**다. 아래 `listening on`
        // 줄을 보고 바로 신호를 보내는 쪽(supervisor·스크립트·테스트)이 있으므로,
        // 등록이 늦으면 그 창에서 daemon이 죽는다. 창이 좁아 재현은 어렵지만
        // 기본 동작이 종료인 신호에 경쟁을 남겨 둘 이유가 없다.
        let hangup = signal_stream(SignalKind::hangup(), "SIGHUP");
        let user1 = signal_stream(SignalKind::user_defined1(), "SIGUSR1");

        // **`listening on`보다 먼저 연다.** (PRD §85)
        //
        // 그 줄을 읽고 바로 움직이는 쪽(supervisor · 스크립트 · 테스트)이 있다.
        // 신호 등록을 앞으로 옮긴 것과 같은 이유다 — 창구가 아직 없는 창을
        // 남기지 않는다.
        //
        // 열지 못하면 **기동을 거부한다.** 창구가 있다고 믿는 쪽이 조용히
        // 실패하는 것보다 낫다.
        let control = match &options.control_socket {
            None => None,
            Some(path) => {
                let socket = nimwake_core::control::ControlSocket::bind(path)
                    .await
                    .map_err(|error| match error.kind() {
                        // 디렉터리가 없는 것은 설정 문제다. 만들어 주지 않는다.
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
                            Failure::config(format!(
                                "failed to open the control socket at {}: {error}",
                                path.display()
                            ))
                        }
                        _ => Failure::unavailable(format!(
                            "failed to open the control socket at {}: {error}",
                            path.display()
                        )),
                    })?;
                Some(tokio::spawn(socket.serve(Arc::clone(&registry))))
            }
        };

        if !addr.ip().is_loopback() {
            // 외부 노출은 opt-in이다. 선택했다는 사실을 남긴다. (PRD §74, §46)
            warn!(%addr, "gateway is reachable beyond loopback; Nimwake provides no TLS or auth");
        }

        info!(%addr, apps = registry.len(), "gateway is listening");
        println!("listening on http://{addr} — every application starts asleep");
        // 이 줄을 읽고 요청을 보내는 쪽이 있다. 버퍼에 남겨 두면 안 된다.
        use std::io::Write;
        let _ = std::io::stdout().flush();

        // `SIGHUP`마다 manifest를 다시 읽는다. (PRD §76)
        //
        // gateway와 같은 수명이다. serve가 끝나면 더 받을 이유가 없다.
        let reloader = tokio::spawn(reload_on_hangup(
            hangup,
            Arc::clone(&registry),
            apps_dir,
            requested.port(),
        ));

        // `SIGUSR1`마다 상태를 로그에 쓴다. (PRD §79)
        let reporter = tokio::spawn(report_on_usr1(user1, Arc::clone(&registry)));

        // 메모리가 부족하면 가장 오래 유휴인 app을 내린다. (PRD §80)
        let evictor = tokio::spawn(evict_under_pressure(
            Arc::clone(&registry),
            Arc::clone(&runtime),
            limits,
        ));

        gateway.serve(shutdown_signal()).await;
        reloader.abort();
        reporter.abort();
        evictor.abort();
        // **socket 파일을 명시적으로 지운다.** (PRD §85)
        //
        // 전에는 `abort()`가 `ControlSocket`을 drop시키는 것에 기댔다. 그런데
        // abort는 취소를 **요청**할 뿐이고 Drop이 언제 도는지는 런타임이
        // 정한다 — 종료 중에는 돌기 전에 process가 끝날 수 있다. CI에서
        // `socket 파일이 5초 안에 지워지지 않았다`로 드러났다.
        //
        // `Drop`은 그대로 둔다 — 비정상 종료에서 그것만이 치울 기회다.
        if let Some(control) = control {
            control.abort();
            if let Some(path) = &options.control_socket {
                if let Err(error) = std::fs::remove_file(path) {
                    // 이미 없으면 할 일이 없다. 다음 기동이 stale로 보고 치운다.
                    if error.kind() != std::io::ErrorKind::NotFound {
                        warn!(path = %path.display(), %error, "failed to remove the control socket");
                    }
                }
            }
        }

        // child process의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10 / PRD §71)
        info!("stopping every application");
        registry.shutdown().await;

        Ok(())
    })
}

/// 메모리가 부족하면 가장 오래 유휴인 application을 하나 내린다. (PRD §80)
///
/// §78은 **새로 깨우지 않는 것**까지였다. 그것만으로는 아무도 쓰지 않는 메모리를
/// 쥔 채 요청을 거절한다 — app 셋이 118MB씩 쥐고 200초째 유휴인데 네 번째
/// 요청이 `503`을 받는다.
///
/// **거절 지점이 아니라 주기적으로 하는 이유**: §78의 검사는 각 application의
/// actor 안에 있고 다른 application을 모른다. 거절 지점에서 하려면 요청 경로가
/// application들을 가로질러 정책을 결정해야 한다. 이쪽은 요청 경로를 건드리지
/// 않고, 기존 handle로 `Stop`을 보낼 뿐이다 — §76의 재적재가 이미 하는 일이다.
#[cfg(unix)]
async fn evict_under_pressure(
    registry: std::sync::Arc<nimwake_core::AppRegistry>,
    runtime: std::sync::Arc<dyn nimwake_core::Runtime>,
    limits: nimwake_core::Limits,
) {
    use tracing::warn;

    // 꺼져 있으면 돌지 않는다. `/proc/meminfo`를 읽지도 않는다. (PRD §78, §80)
    if !limits.checks_memory() {
        return;
    }

    let mut ticker = tokio::time::interval(EVICTION_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        ticker.tick().await;

        // **판단과 정지는 `nimwake_core::eviction::sweep`에 있다.**
        //
        // 여기 두었더니 테스트가 실제 메모리 압박을 만들어야 했고, 개발 기계에서는
        // `echo-server`가 2MB라 임계값을 넘길 수 없었다. core의 한 걸음으로 옮기니
        // `FakeRuntime`으로 결정적으로 검증할 수 있다.
        if let Err(error) = nimwake_core::eviction::sweep(&registry, &runtime, limits).await {
            warn!(%error, "eviction failed");
        }
    }
}

/// 신호 스트림을 등록한다. 실패하면 그 신호 기능만 꺼진다.
///
/// 등록을 **알림(`listening on`)보다 먼저** 해야 한다. `SIGHUP`·`SIGUSR1`의 기본
/// 동작은 프로세스 종료이기 때문이다.
#[cfg(unix)]
fn signal_stream(
    kind: tokio::signal::unix::SignalKind,
    name: &str,
) -> Option<tokio::signal::unix::Signal> {
    match tokio::signal::unix::signal(kind) {
        Ok(stream) => Some(stream),
        Err(error) => {
            error!(%error, signal = name, "failed to listen for a signal; that feature is off");
            None
        }
    }
}

/// `SIGUSR1`마다 지금 상태를 로그에 한 번 쓴다. (PRD §79)
///
/// **돌고 있는 daemon에게 "무슨 버전이고 지금 무엇을 하고 있나"를 물을 창구다.**
///
/// `deploy`는 떠 있는 process를 바꾸지 않으므로 디스크의 바이너리와 돌고 있는
/// 것이 다를 수 있다. 실제로 그 차이를 모르고 72시간 측정을 시작할 뻔했다.
///
/// gateway에 붙이지 않는다. §74는 외부 노출을 opt-in으로 두고 Core는 인증을
/// 제공하지 않는다 — `--listen 0.0.0.0`으로 열면 application 이름과 pid가 LAN의
/// 누구에게나 보인다. 신호는 같은 uid를 요구하므로 새 공격면이 없다.
///
/// 상태를 바꾸지 않는다. `Status`는 actor를 깨우지 않고 현재 상태만 읽는다.
#[cfg(unix)]
async fn report_on_usr1(
    user1: Option<tokio::signal::unix::Signal>,
    registry: std::sync::Arc<nimwake_core::AppRegistry>,
) {
    use tracing::{info, warn};

    let Some(mut user1) = user1 else { return };

    while user1.recv().await.is_some() {
        let apps = registry.apps();

        // 사람이 읽는다. 기계가 읽는 형식을 약속하지 않는다. (PRD §79)
        //
        // `println!`이 아니라 로그로 보내면 timestamp가 붙고 supervisor가 이미
        // 모으고 있는 곳에 들어간다.
        info!(
            version = env!("CARGO_PKG_VERSION"),
            apps = apps.len(),
            "status snapshot",
        );

        for handle in apps {
            match handle.status().await {
                Ok(status) => info!(
                    app = status.name,
                    host = handle.host(),
                    state = %status.state,
                    pid = status.os_pid.map_or_else(|| "-".to_owned(), |pid| pid.to_string()),
                    process = status.process_id.map_or_else(|| "-".to_owned(), |id| id.to_string()),
                    active = status.active_requests,
                    pending = status.pending_waiters,
                    since_ms = status.since.map_or(0, |since| since.as_millis()),
                    // 유휴 시간을 함께 적는다. eviction(§80)이 다음에 무엇을 고를지
                    // 운영자가 알 수 있어야 한다 — 가장 큰 값이 다음 대상이다.
                    idle_ms = status.idle_for.map_or(0, |idle| idle.as_millis()),
                    error = status.last_error.map(|error| error.to_string()).unwrap_or_default(),
                    "  app",
                ),
                // actor가 내려갔으면 그 사실이 상태다. 전체를 멈추지 않는다.
                Err(error) => warn!(
                    app = handle.name(),
                    host = handle.host(),
                    %error,
                    "  app is unreachable",
                ),
            }
        }
    }
}

/// `SIGHUP`마다 manifest를 다시 읽어 차이만 적용한다. (PRD §76)
///
/// **실패해도 daemon은 계속 서빙한다.** 기동 시점의 검증 실패는 종료 코드 `78`이지만
/// (PRD §74), 재적재 실패는 이미 서빙 중인 application을 내릴 이유가 되지 않는다.
///
/// manifest가 하나도 없으면 거부한다. "모두 지웠다"와 "디렉터리를 잘못 건드렸다"를
/// 구분할 수 없고, 후자일 때 전부 내리면 복구가 어렵다.
#[cfg(unix)]
async fn reload_on_hangup(
    hangup: Option<tokio::signal::unix::Signal>,
    registry: std::sync::Arc<nimwake_core::AppRegistry>,
    apps_dir: std::path::PathBuf,
    gateway_port: u16,
) {
    use tracing::{info, warn};

    let Some(mut hangup) = hangup else { return };

    let _ = gateway_port;

    while hangup.recv().await.is_some() {
        info!(dir = %apps_dir.display(), "SIGHUP received; reloading manifests");

        let specs = match registry::load_dir(&apps_dir) {
            Ok(specs) => specs,
            Err(error) => {
                warn!(%error, dir = %apps_dir.display(), "reload failed to read manifests; keeping the current set");
                continue;
            }
        };

        if specs.is_empty() {
            warn!(
                dir = %apps_dir.display(),
                "reload found no manifests; keeping the current set",
            );
            continue;
        }

        match registry.reload(specs).await {
            Ok(outcome) => {
                println!(
                    "reloaded: +{} -{} ~{} ={}",
                    outcome.added.len(),
                    outcome.removed.len(),
                    outcome.replaced.len(),
                    outcome.unchanged.len(),
                );
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            Err(error) => {
                warn!(%error, "reload rejected by validation; keeping the current set");
            }
        }
    }
}

/// `SIGINT` 또는 `SIGTERM`이 오면 완료된다.
#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(signal) => signal,
        Err(error) => {
            error!(%error, "failed to listen for SIGINT");
            return;
        }
    };
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            error!(%error, "failed to listen for SIGTERM");
            return;
        }
    };

    let received = tokio::select! {
        _ = interrupt.recv() => "SIGINT",
        _ = terminate.recv() => "SIGTERM",
    };
    tracing::info!(signal = received, "shutdown signal received");
}

/// Unix 밖에서는 검증까지만 한다.
///
/// `ProcessRuntime`이 Unix 전용이므로 application을 기동할 방법이 없다. (PRD §19, §48)
#[cfg(not(unix))]
fn serve(_specs: Vec<AppSpec>, _options: &Options) -> Result<(), Failure> {
    println!("manifests are valid.");
    println!(
        "serving requires a Unix-like OS: ProcessRuntime is built on POSIX signals (PRD §48)."
    );
    Ok(())
}
