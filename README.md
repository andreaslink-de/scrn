# SCRN

**Snap · Crop · Redact · Note**

SCRN is a self-hosted screenshot sharing tool. The name describes the workflow of
the desktop client: take a screenshot (Snap), select the relevant area (Crop),
blur sensitive details (Redact) and add arrows or highlights (Note). The finished
image is uploaded to your own server and can be shared through a short link.

> **Status:** first release (0.1.0) of the server. The desktop client is
> not available yet; until then the upload API can be used directly.

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
thumbnail under `data/files/<user id>/` (the numeric ID, never the user
name; files of the earlier flat layout are moved there at startup). Public screenshots get a short link;
`expires_in_days` sets its lifetime (default: the user's setting, 0 means
never). `GET /api/v1/screenshots?limit=N` lists the newest screenshots and
`DELETE /api/v1/screenshots/<id>` deletes one.

A public screenshot is served at `<SCRN_BASE_URL>/<short_id>` (also with
`.png`). Private, expired and unknown IDs all return the same 404 page.
Browsers may cache an image for at most five minutes (never beyond the link's
expiry), and each client IP may fetch 300 links per minute.

Each delivery of a short link counts as a view, revalidations by the
browser included; `HEAD` requests do not count. Link previews of messengers
are counted too, as they cannot be told apart reliably. The gallery and the
API (`views`) show the count of the current link; it starts at 0 whenever a
screenshot gets a new link and is dropped when it becomes private.

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

Compose additionally reads `SCRN_IMAGE` (image to run, empty builds it),
`SCRN_PUBLISH_ADDR` (default `127.0.0.1`),
`SCRN_PUBLISH_PORT` (default `8080`), `SCRN_UID` and `SCRN_GID` (default
`1000`, must own `./data`).

## Deployment

SCRN is meant to run behind a reverse proxy that terminates TLS. This
section uses [Nginx Proxy Manager](https://nginxproxymanager.com/) (NPM);
any other proxy works the same way.

### Server

1. Create `.env` next to `compose.yaml` (start from `.env.example`):

   ```sh
   SCRN_BASE_URL=https://scrn.example.org
   SCRN_COOKIE_SECURE=true
   SCRN_TRUSTED_PROXIES=172.18.0.0/16
   SCRN_DEFAULT_QUOTA_MB=1024
   ```

   `SCRN_BASE_URL` is the public address: short links, invitation links and
   passkeys are built from it. Passkeys are bound to its host name.

2. Choose the image. Releases are published as
   `ghcr.io/andreaslink-de/scrn:<version>` (and `latest`); set
   `SCRN_IMAGE=ghcr.io/andreaslink-de/scrn:0.1.0` in `.env` to use one.
   Without `SCRN_IMAGE`, Compose builds the image from the checkout.

3. Start it: `mkdir -p data && docker compose pull && docker compose up -d`
   (or `docker compose up -d --build` without `SCRN_IMAGE`). Then read the
   setup token with `docker compose logs scrn | grep setup_token` and open
   `<SCRN_BASE_URL>/setup`.

To update, change the version in `SCRN_IMAGE`, then run
`docker compose pull && docker compose up -d`. Migrations run
automatically at startup; take a backup first (see below). If the image
on GHCR is private, the server needs `docker login ghcr.io` with a
personal access token that has the `read:packages` scope.

The container runs as an unprivileged user with a read-only root file
system, no capabilities and `no-new-privileges`. By default the port is only
published on `127.0.0.1`.

### Trusted proxies

The server sees the proxy, not the visitor. It trusts `X-Forwarded-For`
only from the addresses in `SCRN_TRUSTED_PROXIES`, so rate limits apply per
visitor and nobody can fake an address by sending the header themselves.
Set it to the address NPM connects from:

- **NPM and SCRN in the same Docker network** (recommended): put both
  containers into one network, for example by adding NPM's network to
  `compose.yaml`:

  ```yaml
  services:
    scrn:
      networks: [npm]
  networks:
    npm:
      external: true
      name: npm_default   # see `docker network ls`
  ```

  Then use the network's subnet (`docker network inspect npm_default`, for
  example `172.18.0.0/16`) as `SCRN_TRUSTED_PROXIES`, and forward NPM to the
  host name `scrn`, port `8080`.
- **NPM on the host or another machine**: publish the port where NPM can
  reach it (`SCRN_PUBLISH_ADDR`) and use NPM's address, for example
  `SCRN_TRUSTED_PROXIES=127.0.0.1` or the Docker bridge gateway
  `172.17.0.1`.

If the value is wrong, all visitors share the proxy's address for rate
limiting; the server keeps working.

### Nginx Proxy Manager

Create a proxy host:

- **Details**: domain `scrn.example.org`, scheme `http`, forward host `scrn`
  (or the host address), forward port `8080`, *Block Common Exploits* on.
  *Websockets Support* is not needed.
- **SSL**: request a Let's Encrypt certificate, enable *Force SSL*,
  *HTTP/2 Support* and *HSTS Enabled*.
- **Advanced**: uploads are up to 20 MiB, so raise nginx' default limit of
  1 MiB:

  ```nginx
  client_max_body_size 25m;
  ```

Check the result with `curl -I https://scrn.example.org/login`: the
response must contain `strict-transport-security`,
`content-security-policy`, `x-frame-options` and `permissions-policy`.

### Limits

| What | Limit |
| --- | --- |
| Login, setup and registration | 20 attempts per IP and 10 failed logins per username in 15 minutes |
| Bad API tokens | 30 per IP in 15 minutes |
| Uploads | 120 per user per hour, 20 MiB and 100 megapixels each, plus the storage quota |
| Public short links | 300 requests per IP per minute |
| Forms and JSON bodies | 64 KiB |
| Request time | 30 seconds, 5 minutes for uploads |

Rate limits are kept in memory and reset when the server restarts.

### Backup and restore

`scripts/backup.sh` creates one archive with a consistent copy of the
database (taken while the server keeps running) and all screenshot files:

```sh
scripts/backup.sh /path/to/backups
# backup written to /path/to/backups/scrn-20261106-120000.tar.gz
```

Run it from the directory with `compose.yaml`, for example daily from cron,
and copy the archives off the machine.

Administrators can also download a backup in the browser: *Administration →
Backup → Download backup*. The archive (`scrn-YYYYMMDD-HHMMSS.tar`) has the
same content, uncompressed because PNG files are compressed already. It is
streamed directly and never stored on the server; only one download runs at
a time.

To restore, stop the server, replace the data directory with the archive's
content and start again:

```sh
docker compose down
mv data data.old                      # keep until the restore is verified
mkdir data
tar -xzf /path/to/backups/scrn-20261106-120000.tar.gz -C data
# a download from the administration: tar -xf scrn-20261106-120000.tar -C data
sudo chown -R "${SCRN_UID:-1000}:${SCRN_GID:-1000}" data
docker compose up -d
```

Sessions, API tokens, passkeys and short links are part of the database,
so the gallery and all links work again as before. Files of uploads made
after the backup was taken are removed by the maintenance job.

## Releases

Releases follow [Semantic Versioning](https://semver.org/) and are listed
in [CHANGELOG.md](CHANGELOG.md). Pushing a tag `vX.Y.Z` that matches the
version in `Cargo.toml` builds the image and publishes it to GHCR.

## Contributing and security

See [CONTRIBUTING.md](CONTRIBUTING.md). Please report vulnerabilities
privately as described in [SECURITY.md](SECURITY.md).

## License

SCRN is licensed under the [MIT License](LICENSE).
