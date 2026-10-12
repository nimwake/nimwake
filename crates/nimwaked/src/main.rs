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
        let registry = AppRegistry::build(
            specs,
            requested.port(),
            Arc::new(ProcessRuntime::new()),
            limits,
        )
        .map_err(|error| Failure::config(format!("registry validation failed: {error}")))?;
        let registry = Arc::new(registry);

        // bind 실패는 시간이 해결할 수 있다. 이전 인스턴스가 아직 port를 쥐고 있을 수 있다.
        let gateway = Gateway::bind(requested, Arc::clone(&registry))
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

        gateway.serve(shutdown_signal()).await;
        reloader.abort();
        reporter.abort();

        // child process의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10 / PRD §71)
        info!("stopping every application");
        registry.shutdown().await;

        Ok(())
    })
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
