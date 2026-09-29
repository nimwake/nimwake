//! Nimwake daemon.
//!
//! `v0.0.1 — Lifecycle Foundation` 범위에서 이 바이너리가 하는 일은
//! **manifest를 읽고 기동 단계에서 검증하는 것**까지다.
//!
//! 검증 실패를 런타임까지 미루면 cold start 시점에야 port 충돌이 드러난다. (PRD §21)
//!
//! - 실제 process 기동(`ProcessRuntime`)은 `v0.0.2`
//! - HTTP gateway와 scale-to-zero 서빙은 `v0.0.3`

use std::path::PathBuf;
use std::process::ExitCode;

use nimwake_core::registry::{self, DEFAULT_GATEWAY_PORT};
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

fn run(apps_dir: &PathBuf) -> Result<(), String> {
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
    println!(
        "serving is not implemented yet: ProcessRuntime lands in v0.0.2, the gateway in v0.0.3."
    );

    Ok(())
}
