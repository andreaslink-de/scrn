//! SCRN server library: configuration, database, HTTP routes and the
//! server lifecycle. The binary in `main.rs` is a thin wrapper around it.

pub mod api;
pub mod auth;
pub mod clock;
pub mod config;
pub mod cookies;
pub mod crypto;
pub mod db;
pub mod error;
pub mod hardening;
pub mod healthcheck;
pub mod html;
pub mod i18n;
pub mod image;
pub mod jobs;
pub mod net;
pub mod passkey;
pub mod prefs;
pub mod rate_limit;
pub mod repo;
pub mod routes;
pub mod short_id;
pub mod shots;
pub mod storage;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use sqlx::SqlitePool;
use tokio::net::TcpListener;

use crate::clock::{Clock, SystemClock};
use crate::rate_limit::RateLimiter;
use crate::repo::Sha256;

pub use config::Config;

/// Login attempts per client IP within [`LOGIN_WINDOW`].
pub const LOGIN_ATTEMPTS_PER_IP: u32 = 20;
/// Failed logins per username within [`LOGIN_WINDOW`].
pub const LOGIN_FAILURES_PER_USER: u32 = 10;
/// Rate limit window for logins in seconds.
pub const LOGIN_WINDOW: i64 = 15 * 60;

/// Failed API authentications per client IP within [`API_FAILURE_WINDOW`].
pub const API_FAILURES_PER_IP: u32 = 30;
pub const API_FAILURE_WINDOW: i64 = 15 * 60;
/// Uploads per user within [`UPLOAD_WINDOW`].
pub const UPLOADS_PER_USER: u32 = 120;
pub const UPLOAD_WINDOW: i64 = 60 * 60;

/// Uploads decoded in parallel.
pub const IMAGE_WORKERS: usize = 2;

/// Shared state passed to all request handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub clock: Arc<dyn Clock>,
    /// Hash of the one-time setup token while no user exists.
    pub setup_token: Arc<Mutex<Option<Sha256>>>,
    pub login_ip_limiter: Arc<RateLimiter>,
    pub login_user_limiter: Arc<RateLimiter>,
    /// Requests to public short links per client IP.
    pub public_ip_limiter: Arc<RateLimiter>,
    /// Failed bearer authentications per client IP.
    pub api_failure_limiter: Arc<RateLimiter>,
    /// Uploads per user.
    pub upload_limiter: Arc<RateLimiter>,
    /// Hash verified for unknown usernames so that the response time does
    /// not reveal whether an account exists.
    pub dummy_password_hash: Arc<str>,
    /// Limits how many uploads are decoded at the same time, which bounds
    /// memory use (a 100-megapixel image needs about 400 MiB).
    pub image_permits: Arc<tokio::sync::Semaphore>,
    /// Passkey support; `None` if the base URL has no domain name.
    pub passkeys: Option<Arc<passkey::Passkeys>>,
}

impl AppState {
    /// Creates a new setup token, replacing any previous one, and returns
    /// it. Only the hash is kept in memory.
    pub fn issue_setup_token(&self) -> Result<String, crypto::CryptoError> {
        let token = crypto::SecretToken::generate()?;
        *self.lock_setup_token() = Some(token.hash);
        Ok(token.token)
    }

    /// True if `token` is the current setup token.
    pub fn check_setup_token(&self, token: &str) -> bool {
        let Some(submitted) = crypto::hash_hex_token(token) else {
            return false;
        };
        self.lock_setup_token()
            .as_ref()
            .is_some_and(|expected| crypto::constant_time_eq(expected, &submitted))
    }

    /// Invalidates the setup token after the admin account was created.
    pub fn clear_setup_token(&self) {
        *self.lock_setup_token() = None;
    }

    fn lock_setup_token(&self) -> std::sync::MutexGuard<'_, Option<Sha256>> {
        self.setup_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
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
    #[error(transparent)]
    Catalog(#[from] i18n::CatalogError),
    #[error(transparent)]
    Crypto(#[from] crypto::CryptoError),
    #[error(transparent)]
    Repo(#[from] repo::RepoError),
    #[error("failed to bind {addr}: {source}")]
    Bind {
        addr: std::net::SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("server error: {0}")]
    Serve(#[source] std::io::Error),
}

/// Creates the data directories and opens the database, using the system
/// clock.
pub async fn init_state(config: Config) -> Result<AppState, ServerError> {
    init_state_with_clock(config, Arc::new(SystemClock)).await
}

/// Like [`init_state`] with an injected clock (for tests).
pub async fn init_state_with_clock(
    config: Config,
    clock: Arc<dyn Clock>,
) -> Result<AppState, ServerError> {
    // Fail at startup, not on the first request, if a catalog is broken.
    i18n::I18n::load()?;

    for dir in [config.data_dir.clone(), config.files_dir()] {
        std::fs::create_dir_all(&dir).map_err(|source| ServerError::DataDir {
            path: dir.display().to_string(),
            source,
        })?;
    }
    let db = db::connect(&config.database_path()).await?;
    jobs::move_legacy_files(&db, &config.files_dir()).await?;
    let passkeys = passkey::build(&config.base_url).map(|w| Arc::new(passkey::Passkeys::new(w)));
    if passkeys.is_none() {
        tracing::warn!(
            base_url = %config.base_url,
            "passkeys are disabled: SCRN_BASE_URL needs a host name, not an IP address"
        );
    }
    let dummy = crypto::SecretToken::generate()?.token;
    let dummy_password_hash: Arc<str> =
        tokio::task::spawn_blocking(move || crypto::hash_password(&dummy))
            .await
            .map_err(|_| crypto::CryptoError::Hash)??
            .into();
    Ok(AppState {
        config: Arc::new(config),
        db,
        clock,
        setup_token: Arc::new(Mutex::new(None)),
        login_ip_limiter: Arc::new(RateLimiter::new(LOGIN_ATTEMPTS_PER_IP, LOGIN_WINDOW)),
        login_user_limiter: Arc::new(RateLimiter::new(LOGIN_FAILURES_PER_USER, LOGIN_WINDOW)),
        public_ip_limiter: Arc::new(RateLimiter::new(
            routes::PUBLIC_REQUESTS_PER_IP,
            routes::PUBLIC_WINDOW,
        )),
        api_failure_limiter: Arc::new(RateLimiter::new(API_FAILURES_PER_IP, API_FAILURE_WINDOW)),
        upload_limiter: Arc::new(RateLimiter::new(UPLOADS_PER_USER, UPLOAD_WINDOW)),
        dummy_password_hash,
        image_permits: Arc::new(tokio::sync::Semaphore::new(IMAGE_WORKERS)),
        passkeys,
    })
}

/// Issues and logs the one-time setup token if no user exists yet.
///
/// This is the only place where a secret is written to the log, on purpose:
/// the operator reads it there to create the first admin account.
pub async fn announce_setup_token(state: &AppState) -> Result<(), ServerError> {
    if repo::users::count(&state.db).await? == 0 {
        let token = state.issue_setup_token()?;
        tracing::warn!(
            setup_url = %format!("{}/setup", state.config.base_url),
            setup_token = %token,
            "no user exists yet: open the setup page and enter this one-time setup token"
        );
    }
    Ok(())
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
    let service = app(state).into_make_service_with_connect_info::<SocketAddr>();
    let result = axum::serve(listener, service)
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
    announce_setup_token(&state).await?;
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
    let maintenance = jobs::spawn(state.clone());
    let result = serve(listener, state, shutdown_signal()).await;
    maintenance.abort();
    result?;
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
