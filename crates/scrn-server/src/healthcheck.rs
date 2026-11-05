//! Minimal HTTP probe used as the Docker `HEALTHCHECK`.
//!
//! The runtime image ships no curl or wget, so the server binary checks
//! itself: `scrn-server healthcheck` requests `/healthz` and exits with 0 on
//! HTTP 200.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TIMEOUT: Duration = Duration::from_secs(3);

/// Errors of the health probe.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("health check timed out")]
    Timeout,
    #[error("health check failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("health check returned an unexpected status line: {0:?}")]
    Status(String),
}

/// Maps a wildcard bind address to the matching loopback address.
pub fn probe_target(bind: SocketAddr) -> SocketAddr {
    let ip = match bind.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, bind.port())
}

/// Requests `GET /healthz` from `addr` and succeeds on HTTP 200.
pub async fn probe(addr: SocketAddr) -> Result<(), ProbeError> {
    tokio::time::timeout(TIMEOUT, probe_inner(addr))
        .await
        .map_err(|_| ProbeError::Timeout)?
}

async fn probe_inner(addr: SocketAddr) -> Result<(), ProbeError> {
    let mut stream = TcpStream::connect(addr).await?;
    stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await?;

    // The status line fits easily into the first bytes of the response.
    let mut buf = [0u8; 256];
    let mut len = 0;
    while len < buf.len() {
        let n = stream.read(&mut buf[len..]).await?;
        if n == 0 {
            break;
        }
        len += n;
        if buf[..len].contains(&b'\n') {
            break;
        }
    }

    let head = String::from_utf8_lossy(&buf[..len]);
    let status_line = head.lines().next().unwrap_or_default().trim().to_owned();
    let mut parts = status_line.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(version), Some("200")) if version.starts_with("HTTP/") => Ok(()),
        _ => Err(ProbeError::Status(status_line)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_bind_maps_to_loopback() {
        assert_eq!(
            probe_target("0.0.0.0:8080".parse().unwrap()),
            "127.0.0.1:8080".parse().unwrap()
        );
        assert_eq!(
            probe_target("[::]:8080".parse().unwrap()),
            "[::1]:8080".parse().unwrap()
        );
        assert_eq!(
            probe_target("192.168.1.2:80".parse().unwrap()),
            "192.168.1.2:80".parse().unwrap()
        );
    }

    #[tokio::test]
    async fn non_200_status_fails() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 512];
            let _ = socket.read(&mut buf).await;
            let _ = socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\n\r\n")
                .await;
        });
        assert!(matches!(probe(addr).await, Err(ProbeError::Status(_))));
    }

    #[tokio::test]
    async fn closed_port_fails() {
        // Bind and drop to get a port that is very likely unused.
        let addr = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap()
        };
        assert!(probe(addr).await.is_err());
    }
}
