//! Client of the SCRN server API (`/api/v1`, see `scrn-proto`).

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use scrn_proto::{
    API_PREFIX, ApiErrorBody, MeResponse, ScreenshotInfo, ScreenshotList, Visibility,
};

use crate::i18n::t;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("network: {0}")]
    Network(String),
    /// An error response of the server with its stable code.
    #[error("server answered {status}: {code}")]
    Server { status: u16, code: String },
    /// Something answered, but not like a SCRN server.
    #[error("unexpected answer: {0}")]
    Unexpected(String),
}

impl ApiError {
    /// A sentence for the user in the system language.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Network(_) => t("error.network"),
            Self::Server { code, .. } => match code.as_str() {
                "unauthorized" => t("error.unauthorized"),
                "quota_exceeded" => t("error.quota"),
                "payload_too_large" | "image_too_large" => t("error.too_large"),
                "too_many_requests" => t("error.rate"),
                _ => t("error.server"),
            },
            Self::Unexpected(_) => t("error.not_scrn"),
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_decode() {
            Self::Unexpected(err.to_string())
        } else {
            // Without the URL: it is harmless, but keeps the log short.
            Self::Network(err.without_url().to_string())
        }
    }
}

pub struct Api {
    client: reqwest::Client,
    base: String,
    token: String,
}

impl Api {
    /// `base` is the normalized server URL (see [`crate::config`]).
    pub fn new(base: &str, token: &str) -> Result<Self, ApiError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("scrn-client/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            // Uploads of up to 20 MiB over a slow uplink.
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{API_PREFIX}{path}", self.base)
    }

    /// The page of a screenshot in the web gallery.
    pub fn gallery_url(base: &str, id: &str) -> String {
        format!("{}/app/shots/{id}", base.trim_end_matches('/'))
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let response = request
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response.json::<T>().await?);
        }
        match response.json::<ApiErrorBody>().await {
            Ok(body) => Err(ApiError::Server {
                status: status.as_u16(),
                code: body.error,
            }),
            Err(_) => Err(ApiError::Unexpected(format!("status {status}"))),
        }
    }

    /// `GET /me`: checks address and token.
    pub async fn me(&self) -> Result<MeResponse, ApiError> {
        self.send(self.client.get(self.url("/me"))).await
    }

    /// `POST /screenshots` with the PNG as body. `expires_in_days` applies
    /// to public uploads only; `None` takes the account default.
    pub async fn upload(
        &self,
        png: Vec<u8>,
        visibility: Visibility,
        expires_in_days: Option<u32>,
    ) -> Result<ScreenshotInfo, ApiError> {
        let mut url = self.url("/screenshots");
        match visibility {
            Visibility::Private => url.push_str("?visibility=private"),
            Visibility::Public => {
                url.push_str("?visibility=public");
                if let Some(days) = expires_in_days {
                    url.push_str(&format!("&expires_in_days={days}"));
                }
            }
        }
        let request = self
            .client
            .post(url)
            .header(CONTENT_TYPE, "image/png")
            .body(png);
        self.send(request).await
    }

    /// The newest screenshot, if there is one.
    pub async fn latest(&self) -> Result<Option<ScreenshotInfo>, ApiError> {
        let list: ScreenshotList = self
            .send(self.client.get(self.url("/screenshots?limit=1")))
            .await?;
        Ok(list.screenshots.into_iter().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A server that answers one request with `status` and `body` and
    /// returns what it received (request line, headers and body).
    async fn serve_once(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data);
                if let Some(end) = text.find("\r\n\r\n") {
                    let len = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&data).into_owned()
        });
        (base, task)
    }

    const SHOT: &str = r#"{"id":"0b6f","visibility":"public","short_url":"https://scrn.dev/AbCd1234","expires_at":null,"width":2,"height":1,"created_at":7,"views":0}"#;

    #[tokio::test]
    async fn upload_sends_the_png_with_token_and_parameters() {
        let (base, server) = serve_once("201 Created", SHOT).await;
        let api = Api::new(&base, "scrn_secret").unwrap();
        let info = api
            .upload(b"PNGDATA".to_vec(), Visibility::Public, Some(7))
            .await
            .unwrap();
        assert_eq!(info.short_url.as_deref(), Some("https://scrn.dev/AbCd1234"));
        let request = server.await.unwrap();
        assert!(
            request.starts_with(
                "POST /api/v1/screenshots?visibility=public&expires_in_days=7 HTTP/1.1"
            )
        );
        let lower = request.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer scrn_secret"));
        assert!(lower.contains("content-type: image/png"));
        assert!(request.ends_with("PNGDATA"));
    }

    #[tokio::test]
    async fn private_uploads_send_no_expiry() {
        let (base, server) = serve_once("201 Created", SHOT).await;
        let api = Api::new(&base, "t").unwrap();
        api.upload(Vec::new(), Visibility::Private, Some(7))
            .await
            .unwrap();
        assert!(
            server
                .await
                .unwrap()
                .starts_with("POST /api/v1/screenshots?visibility=private HTTP/1.1")
        );
    }

    #[tokio::test]
    async fn server_errors_keep_their_code() {
        let (base, _server) = serve_once(
            "413 Payload Too Large",
            r#"{"error":"quota_exceeded","message":"full","used_bytes":5,"quota_bytes":4}"#,
        )
        .await;
        let err = Api::new(&base, "t")
            .unwrap()
            .upload(Vec::new(), Visibility::Private, None)
            .await
            .unwrap_err();
        assert!(matches!(&err, ApiError::Server { status: 413, code } if code == "quota_exceeded"));
        assert_eq!(err.describe(), t("error.quota"));
    }

    #[tokio::test]
    async fn other_servers_are_recognized() {
        let (base, _server) = serve_once("404 Not Found", "<html>nope</html>").await;
        let err = Api::new(&base, "t").unwrap().me().await.unwrap_err();
        assert!(matches!(err, ApiError::Unexpected(_)));
        let (base, _server) = serve_once("200 OK", "<html>login</html>").await;
        let err = Api::new(&base, "t").unwrap().me().await.unwrap_err();
        assert!(matches!(err, ApiError::Unexpected(_)), "{err:?}");
    }

    #[tokio::test]
    async fn latest_asks_for_one_screenshot() {
        let (base, server) = serve_once("200 OK", &format!(r#"{{"screenshots":[{SHOT}]}}"#)).await;
        let latest = Api::new(&base, "t").unwrap().latest().await.unwrap();
        assert_eq!(latest.map(|s| s.id).as_deref(), Some("0b6f"));
        assert!(
            server
                .await
                .unwrap()
                .starts_with("GET /api/v1/screenshots?limit=1 HTTP/1.1")
        );
        let (base, _server) = serve_once("200 OK", r#"{"screenshots":[]}"#).await;
        assert!(
            Api::new(&base, "t")
                .unwrap()
                .latest()
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn unreachable_servers_are_network_errors() {
        // Bind and drop to get a port nobody listens on.
        let port = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = Api::new(&format!("http://127.0.0.1:{port}"), "t")
            .unwrap()
            .me()
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::Network(_)), "{err:?}");
    }

    #[test]
    fn gallery_links_point_to_the_shot_page() {
        assert_eq!(
            Api::gallery_url("https://scrn.dev/", "abc"),
            "https://scrn.dev/app/shots/abc"
        );
    }
}
