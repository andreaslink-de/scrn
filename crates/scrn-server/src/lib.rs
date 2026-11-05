//! SCRN server library: configuration, database, HTTP routes and the
//! server lifecycle. The binary in `main.rs` is a thin wrapper around it.

pub mod config;
pub mod db;
pub mod healthcheck;
pub mod net;
pub mod routes;

use std::future::Future;
use std::sync::Arc;

use sqlx::SqlitePool;
use tokio::net::TcpListener;

pub use config::Config;

/// Shared state passed to all request handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
}

/// Errors that stop the server during startup or while running.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("failed to create data directory {path}: {source}")]
    DataDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Db(#[from] db::DbError),
    #[error("failed to bind {addr}: {source}")]
    Bind {
        addr: std::net::SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("server error: {0}")]
    Serve(#[source] std::io::Error),
}

/// Creates the data directories and opens the database.
pub async fn init_state(config: Config) -> Result<AppState, ServerError> {
    for dir in [config.data_dir.clone(), config.files_dir()] {
        std::fs::create_dir_all(&dir).map_err(|source| ServerError::DataDir {
            path: dir.display().to_string(),
            source,
        })?;
    }
    let db = db::connect(&config.database_path()).await?;
    Ok(AppState {
        config: Arc::new(config),
        db,
    })
}

/// Builds the application router.
pub fn app(state: AppState) -> axum::Router {
    routes::router(state)
}

/// Serves the application on `listener` until `shutdown` resolves, then
/// waits for in-flight requests and closes the database pool.
pub async fn serve<F>(
    listener: TcpListener,
    state: AppState,
    shutdown: F,
) -> Result<(), ServerError>
where
    F: Future<Output = ()> + Send + 'static,
{
    let db = state.db.clone();
    let result = axum::serve(listener, app(state))
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(ServerError::Serve);
    db.close().await;
    result
}

/// Binds the configured address and runs the server until SIGINT or SIGTERM.
pub async fn run(config: Config) -> Result<(), ServerError> {
    let bind = config.bind;
    let state = init_state(config).await?;
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|source| ServerError::Bind { addr: bind, source })?;
    tracing::info!(
        addr = %bind,
        base_url = %state.config.base_url,
        data_dir = %state.config.data_dir.display(),
        cookie_secure = state.config.cookie_secure,
        trusted_proxies = state.config.trusted_proxies.nets().len(),
        "scrn-server listening"
    );
    serve(listener, state, shutdown_signal()).await?;
    tracing::info!("scrn-server stopped");
    Ok(())
}

/// Resolves on Ctrl+C or, on Unix, on SIGTERM (sent by `docker stop`).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "failed to listen for Ctrl+C");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
    tracing::info!("shutdown signal received, finishing open requests");
}
