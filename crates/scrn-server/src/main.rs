//! SCRN server binary.
//!
//! `scrn-server` runs the server, `scrn-server healthcheck` probes a running
//! instance (used by the Docker `HEALTHCHECK`), `scrn-server backup
//! <file>` writes a consistent copy of the database (used by
//! `scripts/backup.sh`) and `scrn-server reset-link <username>` prints a
//! password reset link, the way back in for an admin who lost access.

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
        Some("backup") => match args.get(1) {
            Some(target) if args.len() == 2 => run_backup(target).await,
            _ => {
                eprintln!("usage: scrn-server backup <file>");
                ExitCode::from(2)
            }
        },
        Some("reset-link") => match args.get(1) {
            Some(username) if args.len() == 2 => run_reset_link(username).await,
            _ => {
                eprintln!("usage: scrn-server reset-link <username>");
                ExitCode::from(2)
            }
        },
        Some("--version" | "-V") => {
            println!("scrn-server {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!(
                "unknown argument {other:?}; usage: scrn-server [healthcheck | backup <file> | reset-link <username> | --version]"
            );
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

async fn run_backup(target: &str) -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("invalid configuration: {err}");
            return ExitCode::FAILURE;
        }
    };
    let pool = match scrn_server::db::connect(&config.database_path()).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let result = scrn_server::db::backup(&pool, std::path::Path::new(target)).await;
    pool.close().await;
    match result {
        Ok(()) => {
            println!("database backup written to {target}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

async fn run_reset_link(username: &str) -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("invalid configuration: {err}");
            return ExitCode::FAILURE;
        }
    };
    let pool = match scrn_server::db::connect(&config.database_path()).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    let now = scrn_server::clock::Clock::now(&scrn_server::clock::SystemClock);
    let result = scrn_server::issue_cli_reset_link(&pool, &config, username, now).await;
    pool.close().await;
    match result {
        Ok(Some(link)) => {
            println!("Password reset link for {username}, valid once within 24 hours:\n{link}");
            ExitCode::SUCCESS
        }
        Ok(None) => {
            eprintln!("no user named {username:?}");
            ExitCode::FAILURE
        }
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
