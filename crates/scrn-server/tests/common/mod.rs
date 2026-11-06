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
use axum::http::{HeaderMap, Request, StatusCode};
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

/// Creates an API token for `user` directly in the database.
pub async fn api_token(server: &TestServer, user: &User) -> String {
    let secret = crypto::ApiTokenSecret::generate().unwrap();
    scrn_server::repo::api_tokens::create(&server.state.db, user.id, "test", &secret.hash, START)
        .await
        .unwrap();
    secret.token
}

impl TestServer {
    /// An API client for `user` with a fresh token.
    pub async fn api_client(&self, user: &User) -> Client {
        let token = api_token(self, user).await;
        let mut client = self.client("198.51.100.1");
        client
            .headers
            .push(("authorization".into(), format!("Bearer {token}")));
        client
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
    pub headers: HeaderMap,
    /// The body as text (lossy for binary responses).
    pub body: String,
    pub bytes: Vec<u8>,
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

    /// Sends a request with a raw body and an optional content type.
    pub async fn send_raw(
        &mut self,
        method: &str,
        path: &str,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> TestResponse {
        let mut builder = self.request(method, path);
        if let Some(content_type) = content_type {
            builder = builder.header(CONTENT_TYPE, content_type);
        }
        let request = builder.body(Body::from(body)).unwrap();
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
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        TestResponse {
            status,
            location,
            set_cookies,
            headers,
            body: String::from_utf8_lossy(&bytes).into_owned(),
            bytes: bytes.to_vec(),
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

/// An RGB test image with a text chunk that must not survive the upload.
pub fn png_image(width: u32, height: u32) -> Vec<u8> {
    let pixels: Vec<u8> = (0..width * height * 3).map(|i| (i % 251) as u8).collect();
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .add_text_chunk("Comment".into(), "secret-metadata".into())
        .unwrap();
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    out
}

/// Uploads `body` as PNG through the API.
pub async fn upload(api: &mut Client, query: &str, body: Vec<u8>) -> TestResponse {
    api.send_raw(
        "POST",
        &format!("/api/v1/screenshots{query}"),
        Some("image/png"),
        body,
    )
    .await
}
