//! SCRN server binary.
//!
//! `scrn-server` runs the server, `scrn-server healthcheck` probes a running
//! instance (used by the Docker `HEALTHCHECK`).

use std::io::IsTerminal;
use std::process::ExitCode;

use scrn_server::{Config, healthcheck};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_server().await,
        Some("healthcheck") => run_healthcheck().await,
        Some("--version" | "-V") => {
            println!("scrn-server {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown argument {other:?}; usage: scrn-server [healthcheck | --version]");
            ExitCode::from(2)
        }
    }
}

async fn run_server() -> ExitCode {
    init_tracing();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(error = %err, "invalid configuration");
            return ExitCode::FAILURE;
        }
    };
    match scrn_server::run(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = %err, "scrn-server failed");
            ExitCode::FAILURE
        }
    }
}

async fn run_healthcheck() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("invalid configuration: {err}");
            return ExitCode::FAILURE;
        }
    };
    match healthcheck::probe(healthcheck::probe_target(config.bind)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

/// Logs to stdout. `RUST_LOG` sets the filter (default `info`),
/// `SCRN_LOG_FORMAT=json` switches to JSON lines. Colors are used only when
/// stdout is a terminal, so `docker logs` stays plain text.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let json = std::env::var("SCRN_LOG_FORMAT").is_ok_and(|v| v.eq_ignore_ascii_case("json"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(std::io::stdout().is_terminal());
    if json {
        builder.json().init();
    } else {
        builder.init();
    }
}
