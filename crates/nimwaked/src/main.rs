//! Nimwake daemon.
//!
//! `v0.0.3 — HTTP + Scale-to-Zero` 범위에서 이 바이너리가 하는 일은 다음과 같다.
//!
//! ```text
//! manifest 읽기 → 기동 단계 검증 → registry 구성 → gateway serve → 종료 시 정리
//! ```
//!
//! 검증 실패를 런타임까지 미루면 cold start 시점에야 port 충돌이 드러난다. (PRD §21)
//!
//! serving은 Unix에서만 가능하다. `ProcessRuntime`이 POSIX 신호 위에 서 있기 때문이다. (PRD §19, §48)
//! 다른 플랫폼에서는 manifest 검증까지만 수행한다.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nimwake_core::registry::{self, DEFAULT_GATEWAY_PORT};
use nimwake_core::AppSpec;
use tracing::error;

const DEFAULT_APPS_DIR: &str = "apps";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let apps_dir = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from(DEFAULT_APPS_DIR), PathBuf::from);

    match run(&apps_dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            error!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn run(apps_dir: &Path) -> Result<(), String> {
    let specs = load(apps_dir)?;

    println!(
        "nimwaked {} — {} application(s) validated, gateway port {DEFAULT_GATEWAY_PORT}",
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

    serve(specs)
}

fn load(apps_dir: &Path) -> Result<Vec<AppSpec>, String> {
    let specs = registry::load_dir(apps_dir).map_err(|error| {
        format!(
            "failed to load manifests from `{}`: {error}",
            apps_dir.display()
        )
    })?;

    if specs.is_empty() {
        return Err(format!("no manifests found under `{}`", apps_dir.display()));
    }

    registry::validate(&specs, DEFAULT_GATEWAY_PORT)
        .map_err(|error| format!("manifest validation failed: {error}"))?;

    Ok(specs)
}

#[cfg(unix)]
fn serve(specs: Vec<AppSpec>) -> Result<(), String> {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use nimwake_core::runtime::ProcessRuntime;
    use nimwake_core::{AppRegistry, Gateway};
    use tracing::info;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?;

    runtime.block_on(async move {
        let registry =
            AppRegistry::build(specs, DEFAULT_GATEWAY_PORT, Arc::new(ProcessRuntime::new()))
                .map_err(|error| format!("registry validation failed: {error}"))?;
        let registry = Arc::new(registry);

        let addr = SocketAddr::from(([127, 0, 0, 1], DEFAULT_GATEWAY_PORT));
        let gateway = Gateway::bind(addr, Arc::clone(&registry))
            .await
            .map_err(|error| format!("failed to bind {addr}: {error}"))?;

        info!(%addr, apps = registry.len(), "gateway is listening");
        println!("listening on http://{addr} — every application starts asleep");

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
            tracing::error!(%error, "failed to listen for SIGINT");
            return;
        }
    };
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::error!(%error, "failed to listen for SIGTERM");
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
fn serve(_specs: Vec<AppSpec>) -> Result<(), String> {
    println!("manifests are valid.");
    println!(
        "serving requires a Unix-like OS: ProcessRuntime is built on POSIX signals (PRD §48)."
    );
    Ok(())
}
