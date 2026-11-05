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

## License

SCRN is licensed under the [MIT License](LICENSE).
