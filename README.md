# SCRN

**Snap · Crop · Redact · Note**

SCRN is a self-hosted screenshot sharing tool. The name describes the workflow of
the desktop client: take a screenshot (Snap), select the relevant area (Crop),
blur sensitive details (Redact) and add arrows or highlights (Note). The finished
image is uploaded to your own server and can be shared through a short link.

> **Status:** early development. Nothing here is ready for production use yet.

## Components

- **Server** (`crates/scrn-server`): a Rust HTTP server (axum, SQLite) that runs in
  Docker, stores screenshots, serves public short links and provides a web gallery.
- **Shared types** (`crates/scrn-proto`): API request and response types used by
  the server and the client.
- **Desktop client** (`client/`, planned): a Tauri app with a tray icon, area
  selection, an annotation editor and upload via API token.

Screenshots are private by default. Only public screenshots get a short link, and
public links can expire.

## Development

Requirements: Rust stable (with `rustfmt` and `clippy`) and Docker with the
Compose plugin.

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The implementation plan lives in [docs/PLAN.md](docs/PLAN.md).

## Running with Docker Compose

```sh
cp .env.example .env   # adjust as needed
mkdir -p data          # must be owned by SCRN_UID:SCRN_GID (default 1000:1000)
docker compose up --build -d
curl http://127.0.0.1:8080/healthz
```

The container runs as an unprivileged user with a read-only root filesystem.
All persistent data (the SQLite database `scrn.db` and the `files/` directory)
lives in `./data`, mounted at `/data`. Database migrations run automatically on
startup. The port is published on `127.0.0.1` only; put a reverse proxy with
TLS in front of it.

### Configuration

All settings are environment variables, documented in
[.env.example](.env.example):

| Variable | Default | Meaning |
| --- | --- | --- |
| `SCRN_BASE_URL` | `http://localhost:8080` | Public base URL, used for short links |
| `SCRN_BIND` | `0.0.0.0:8080` | Listen address |
| `SCRN_DATA_DIR` | `/data` | Database and screenshot files |
| `SCRN_COOKIE_SECURE` | `true` | `Secure` flag on session cookies |
| `SCRN_TRUSTED_PROXIES` | empty | IPs or CIDR ranges allowed to set `X-Forwarded-*` |
| `RUST_LOG` | `info` | Log filter |
| `SCRN_LOG_FORMAT` | `text` | `text` or `json` |

Compose additionally reads `SCRN_PUBLISH_ADDR`, `SCRN_PUBLISH_PORT`,
`SCRN_UID` and `SCRN_GID`.

## License

SCRN is licensed under the [MIT License](LICENSE).
