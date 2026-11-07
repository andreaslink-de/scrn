# SCRN

SCRN stands for Snap · Crop · Redact · Note, the workflow of the client. Self-hosted screenshot sharing: a Rust server (axum, SQLite) running in Docker and a Tauri desktop client. License: MIT.

## Language rules

- All code comments, identifiers, commit messages, docs, log messages and API error messages are written in English.
- The web UI is bilingual (English is the default, German is the second language). UI strings live in translation catalogs under `crates/scrn-server/locales`, never hard-coded in templates.

## Layout

- `crates/scrn-proto`: shared API types (serde)
- `crates/scrn-server`: HTTP server, web UI, storage, migrations
- `client/`: Tauri desktop client (added later)
- `docs/`: design notes

## Commands

- Format: `cargo fmt --all`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Test: `cargo test --workspace`
- Run locally: `docker compose up --build`
- Migrations live in `crates/scrn-server/migrations`. Never edit an applied migration, add a new one.

## Engineering rules

- Work in small steps, one branch per step. Keep fmt, clippy and tests green before finishing a step.
- No `unwrap()` or `expect()` in request paths. Use typed errors (thiserror) and map them to HTTP responses.
- Every new behavior gets tests (unit tests plus integration tests against a temporary SQLite database).
- Keep dependencies minimal and justify each new crate in the commit message.
- Docker builds must not need a running database: use runtime-checked sqlx queries, or commit the `.sqlx` offline data (`cargo sqlx prepare --workspace`) and build with `SQLX_OFFLINE=true`.
- Do not change unrelated code. If a requirement is unclear, ask before guessing.

## Security rules (non-negotiable)

- Passwords: argon2id. API tokens: 256-bit random, store only the SHA-256 hash, show the token once.
- Session cookies: HttpOnly, Secure (configurable for local development), SameSite=Lax. CSRF protection on every state-changing form request.
- Private screenshots must never be reachable through a public short URL. Short IDs exist only for public screenshots.
- Uploads: accept PNG only, decode with size and dimension limits, re-encode to strip metadata, never trust the client content type.
- Serve user content with `X-Content-Type-Options: nosniff` and a restrictive CSP.
- Never log tokens, passwords or session IDs.
- Storage quotas are enforced inside the same database transaction that stores the upload. Lowering a quota never deletes data, it only blocks new uploads.
- Reserved route prefixes (`api`, `app`, `static`, `healthz`, ...) must never collide with short IDs.
- The server runs behind a reverse proxy. Trust `X-Forwarded-*` headers only from configured proxy addresses.

## Configuration

- All configuration comes from environment variables, documented in `.env.example`. No secrets in the repo.
- Persistent data lives in `/data` (SQLite file and screenshot files) as a Docker volume.

## Git

- Conventional commits: `feat:`, `fix:`, `chore:`, `docs:`, `test:`.
- Never commit `.env`, the data directory or build output.
