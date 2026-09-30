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
    let fallback = if json {
        "info,nimwake::request=debug"
    } else {
        "info"
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(fallback));

    if json {
        tracing_subscriber::fmt()
            .json()
            .with_env_filter(filter)
            .with_current_span(false)
            .with_span_list(false)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
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

    use nimwake_core::runtime::ProcessRuntime;
    use nimwake_core::{AppRegistry, Gateway};
    use tracing::{info, warn};

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| Failure::other(format!("failed to start the async runtime: {error}")))?;

    let requested = options.listen;

    runtime.block_on(async move {
        let registry = AppRegistry::build(specs, requested.port(), Arc::new(ProcessRuntime::new()))
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

        if !addr.ip().is_loopback() {
            // 외부 노출은 opt-in이다. 선택했다는 사실을 남긴다. (PRD §74, §46)
            warn!(%addr, "gateway is reachable beyond loopback; Nimwake provides no TLS or auth");
        }

        info!(%addr, apps = registry.len(), "gateway is listening");
        println!("listening on http://{addr} — every application starts asleep");
        // 이 줄을 읽고 요청을 보내는 쪽이 있다. 버퍼에 남겨 두면 안 된다.
        use std::io::Write;
        let _ = std::io::stdout().flush();

        gateway.serve(shutdown_signal()).await;

        // child process의 수명은 daemon의 수명을 넘지 않는다. (Invariant 10 / PRD §71)
        info!("stopping every application");
        registry.shutdown().await;

        Ok(())
    })
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
