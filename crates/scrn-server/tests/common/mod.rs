//! Test harness: a server with a temporary database and a manual clock, and
//! browser-like clients with their own cookie jar and IP address.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::header::{CONTENT_TYPE, COOKIE, LOCATION, SET_COOKIE};
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use scrn_server::clock::ManualClock;
use scrn_server::repo::users::{self, NewUser, Role, User};
use scrn_server::{AppState, Config, app, crypto, init_state_with_clock};
use tempfile::TempDir;
use tower::ServiceExt;

pub const START: i64 = 1_800_000_000;
pub const PASSWORD: &str = "correct horse battery staple";

pub struct TestServer {
    pub state: AppState,
    pub clock: Arc<ManualClock>,
    _dir: TempDir,
}

impl TestServer {
    pub async fn new() -> Self {
        Self::with_env(&[]).await
    }

    pub async fn with_env(extra: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut vars: HashMap<String, String> = HashMap::from([
            (
                "SCRN_DATA_DIR".to_owned(),
                dir.path().to_string_lossy().into_owned(),
            ),
            ("SCRN_COOKIE_SECURE".to_owned(), "false".to_owned()),
            ("SCRN_BASE_URL".to_owned(), "https://scrn.test".to_owned()),
        ]);
        for (k, v) in extra {
            vars.insert((*k).to_owned(), (*v).to_owned());
        }
        let config = Config::from_lookup(|name| vars.get(name).cloned()).unwrap();
        let clock = Arc::new(ManualClock::new(START));
        let state = init_state_with_clock(config, clock.clone()).await.unwrap();
        Self {
            state,
            clock,
            _dir: dir,
        }
    }

    /// A client with an empty cookie jar, connecting from `ip`.
    pub fn client(&self, ip: &str) -> Client {
        let addr: SocketAddr = format!("{ip}:40000").parse().unwrap();
        Client {
            router: app(self.state.clone()).layer(MockConnectInfo(addr)),
            cookies: HashMap::new(),
            headers: Vec::new(),
        }
    }

    /// Creates a user directly in the database.
    pub async fn create_user(&self, name: &str, role: Role) -> User {
        let hash = crypto::hash_password(PASSWORD).unwrap();
        users::create(
            &self.state.db,
            &NewUser {
                username: name,
                password_hash: Some(&hash),
                role,
            },
            START,
        )
        .await
        .unwrap()
    }
}

pub struct Client {
    router: Router,
    pub cookies: HashMap<String, String>,
    /// Extra headers sent with every request.
    pub headers: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct TestResponse {
    pub status: StatusCode,
    pub location: Option<String>,
    pub set_cookies: Vec<String>,
    pub body: String,
}

impl TestResponse {
    /// The CSRF token of the first form on the page.
    pub fn csrf(&self) -> String {
        let marker = r#"name="csrf" value=""#;
        let start = self.body.find(marker).expect("page has a CSRF field") + marker.len();
        let end = self.body[start..].find('"').unwrap();
        self.body[start..start + end].to_owned()
    }

    pub fn assert_redirect(&self, to: &str) {
        assert_eq!(self.status, StatusCode::SEE_OTHER, "body: {}", self.body);
        assert_eq!(self.location.as_deref(), Some(to));
    }
}

impl Client {
    pub async fn get(&mut self, path: &str) -> TestResponse {
        let request = self.request("GET", path).body(Body::empty()).unwrap();
        self.send(request).await
    }

    pub async fn post(&mut self, path: &str, fields: &[(&str, &str)]) -> TestResponse {
        let body = fields
            .iter()
            .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let request = self
            .request("POST", path)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        self.send(request).await
    }

    /// GET a form page, then POST it with the page's CSRF token.
    pub async fn submit(
        &mut self,
        page: &str,
        action: &str,
        fields: &[(&str, &str)],
    ) -> TestResponse {
        let csrf = self.get(page).await.csrf();
        let mut all = vec![("csrf", csrf.as_str())];
        all.extend_from_slice(fields);
        self.post(action, &all).await
    }

    pub async fn login(&mut self, username: &str, password: &str) -> TestResponse {
        self.submit(
            "/login",
            "/login",
            &[("username", username), ("password", password)],
        )
        .await
    }

    fn request(&self, method: &str, path: &str) -> axum::http::request::Builder {
        let mut builder = Request::builder().method(method).uri(path);
        if !self.cookies.is_empty() {
            let cookie = self
                .cookies
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("; ");
            builder = builder.header(COOKIE, cookie);
        }
        for (name, value) in &self.headers {
            builder = builder.header(name, value);
        }
        builder
    }

    async fn send(&mut self, request: Request<Body>) -> TestResponse {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let location = response
            .headers()
            .get(LOCATION)
            .map(|v| v.to_str().unwrap().to_owned());
        let set_cookies: Vec<String> = response
            .headers()
            .get_all(SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect();
        for cookie in &set_cookies {
            let (pair, attrs) = cookie.split_once(';').unwrap_or((cookie, ""));
            let (name, value) = pair.split_once('=').unwrap();
            if attrs.contains("Max-Age=0") {
                self.cookies.remove(name);
            } else {
                self.cookies.insert(name.to_owned(), value.to_owned());
            }
        }
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        TestResponse {
            status,
            location,
            set_cookies,
            body: String::from_utf8(bytes.to_vec()).unwrap(),
        }
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
