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

### First start

On the first start without any user account, the server writes a one-time
setup token to its log:

```sh
docker compose logs scrn | grep setup_token
```

Open `<SCRN_BASE_URL>/setup`, enter the token and create the administrator
account (password at least 12 characters). The setup page is locked
afterwards. A restart before the setup is finished prints a new token.

New users can only register through an invitation link: create one on the
administration page (`/admin`), it is single-use and expires after the chosen
number of days. Administrators can disable accounts, which also ends their
sessions.

### Web interface

The gallery at `/app` shows all screenshots of the logged-in user with a
filter (all, public, private). Public cards show the short link with a copy
button. The detail view switches the visibility (making an image private
deletes its short link for good; making it public again creates a new one),
changes the expiry of a public link and deletes the screenshot.

The web UI speaks German (default) and English and has a dark (default) and a
light theme. Logged-in users choose both, plus the default expiry of public
links, under *Settings* (`/app/settings`); the page also shows the storage
usage and manages API tokens. Logged-out pages follow the browser language
and offer a switch in the footer. Administrators see each user's storage
usage on `/admin` and can set a per-user quota in MiB (empty for the server
default, 0 for unlimited).

### Passkeys

Besides the password, users can sign in with passkeys (Windows Hello,
Touch ID, a phone, a security key or a password manager). They are added,
renamed and deleted on the settings page; the login page offers
"Sign in with a passkey" after entering the username. The password stays as
a fallback, and an account's last way to sign in cannot be deleted.

The relying party is derived from `SCRN_BASE_URL`: its host name is the
relying party ID and the URL is the expected origin. Passkeys are bound to
that host name, so changing it later makes existing passkeys unusable.
`http://localhost:8080` works for local development; with an IP address as
base URL, passkeys are switched off.

To test by hand in a browser:

1. Start the server with `SCRN_BASE_URL=http://localhost:8080` and open
   `http://localhost:8080` (not `127.0.0.1`).
2. Sign in with the password, open *Settings*, enter a name under
   *Passkeys* and choose *Add passkey*. The browser asks for Windows Hello,
   Touch ID, a phone or a password manager.
3. Sign out, enter the username on the login page and choose
   *Sign in with a passkey*.

Without a real authenticator, Chrome's DevTools offer a virtual one: open
DevTools, then *More tools* → *WebAuthn*, enable the virtual authenticator
environment and add a `ctap2` authenticator with resident key and user
verification.

### API tokens

The desktop client authenticates with an API token. Create one per device on
the *Settings* page (`/app/settings`). The token (`scrn_…`) is shown only
once; the server stores just its SHA-256 hash. Test it with:

```sh
curl -H "Authorization: Bearer scrn_…" <SCRN_BASE_URL>/api/v1/me
```

Revoked tokens and tokens of disabled accounts are rejected with 401.

### Uploading

```sh
curl -X POST -H "Authorization: Bearer scrn_…" -H "Content-Type: image/png" \
  --data-binary @shot.png "<SCRN_BASE_URL>/api/v1/screenshots?visibility=public"
```

Uploads are PNG only (at most 20 MiB and 100 megapixels). The server decodes
and re-encodes every image, which removes all metadata, and stores it with a
thumbnail under `data/files`. Public screenshots get a short link;
`expires_in_days` sets its lifetime (default: the user's setting, 0 means
never). `GET /api/v1/screenshots?limit=N` lists the newest screenshots and
`DELETE /api/v1/screenshots/<id>` deletes one.

A public screenshot is served at `<SCRN_BASE_URL>/<short_id>` (also with
`.png`). Private, expired and unknown IDs all return the same 404 page.
Browsers may cache an image for at most five minutes (never beyond the link's
expiry), and each client IP may fetch 300 links per minute.

Expired links stop working immediately. A maintenance job runs at startup
and every five minutes: it turns expired public screenshots into private
ones (they stay in the gallery), deletes expired sessions and removes image
files without a database entry once they are an hour old.

Each user has a storage quota (`SCRN_DEFAULT_QUOTA_MB` unless set per user,
0 means unlimited). Uploads beyond it fail with 413 and the code
`quota_exceeded`; lowering a quota never deletes anything.

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
| `SCRN_DEFAULT_QUOTA_MB` | `1024` | Storage quota per user in MiB, 0 for unlimited |
| `RUST_LOG` | `info` | Log filter |
| `SCRN_LOG_FORMAT` | `text` | `text` or `json` |

Compose additionally reads `SCRN_PUBLISH_ADDR`, `SCRN_PUBLISH_PORT`,
`SCRN_UID` and `SCRN_GID`.

## License

SCRN is licensed under the [MIT License](LICENSE).
