# SCRN – Umsetzungsplan (Server zuerst)

Nov 5, 2026 · @Andreas Link

## Überblick und Entscheidungen

SCRN (Snap · Crop · Redact · Note) besteht aus einem Docker-Server in Rust und einem Desktop-Client mit Tauri. Dieser Plan setzt zuerst den Server um, jeder Schritt lässt sich einzeln mit Claude Code ausführen.

| Bereich | Entscheidung |
| --- | --- |
| Repo und Lizenz | GitHub-Repo, später öffentlich, MIT-Lizenz. Code-Kommentare, Commit-Messages und Repo-Doku auf Englisch |
| Server | Rust mit axum, SQLite (sqlx), Bilder als Dateien in einem Volume, ein Container, Start mit `docker compose`, TLS durch Nginx Proxy Manager |
| Web-UI | Serverseitig gerendert (Templates + htmx). Deutsch und Englisch, Default Deutsch. Dark und Light, Default Dark. Beides pro User gespeichert |
| Login | Benutzername und Passwort (argon2), Passkeys optional. Registrierung nur per Einladung des Admins |
| API-Tokens | Mehrere pro User, nur als Hash gespeichert, einmal angezeigt, widerrufbar |
| Sichtbarkeit | Privat: nur in der Galerie nach Login, nie per Kurzlink. Öffentlich: Kurzlink `https://scrn.dev/<id>` mit 8 Zeichen Base62 |
| Ablauf | Öffentliche Links können ablaufen. Standard pro User, pro Bild nachträglich änderbar |
| Galerie | Raster mit Filter (alle, öffentlich, privat), Löschen, und bei öffentlichen Bildern der Kurzlink sichtbar und kopierbar |
| Client | Tauri v2 mit Tray, Freeze-Frame-Auswahl, Editor für Pfeil, Markierung und Blur, Upload mit Token |
| Letzter Link | Tray-Menü „Letzten Link kopieren“, Befehl `scrn copy-last`, Verlauf der letzten Links. Fallback: Abfrage beim Server |
| Linux-Trigger | Tray-Menü und Befehl `scrn capture`, den du im Desktop auf eine Taste legst. Portal-Hotkey erst ab GNOME 48 |
| Speicherkontingent | Pro User festgelegt, vom Admin jederzeit änderbar. Serverstandard per Umgebungsvariable, 0 bedeutet unbegrenzt. Bei Überschreitung werden neue Uploads abgelehnt, vorhandene Bilder bleiben |

## Voraussetzungen und Repo-Setup

Mit einem leeren GitHub-Repo, Rust, Docker und Claude Code im Repo-Ordner bist du startklar.

1. **GitHub-Repo anlegen**, z. B. `scrn`, leer und zunächst privat. Keine README und keine Lizenz anlegen lassen, beides erzeugt Claude Code in Schritt S0.
2. **Lokal klonen:** `git clone git@github.com:<dein-user>/scrn.git` und in den Ordner wechseln.
3. **Toolchain prüfen:** Rust stable über rustup (mit `rustfmt` und `clippy`), Docker Engine mit Compose-Plugin, git. Unter Ubuntu zusätzlich `build-essential pkg-config libssl-dev`, weil die Passkey-Bibliothek OpenSSL nutzt.
4. **Claude Code im Repo-Ordner starten** und als Erstes die `CLAUDE.md` aus dem nächsten Abschnitt anlegen. Sie gilt dann für alle weiteren Schritte.
5. **Pro Schritt ein Branch** (`s1-skeleton`, `s2-datamodel` usw.): Prompt einfügen, Abnahme prüfen, mergen. Mehr dazu im Abschnitt „Arbeitsweise“.

Vor dem Umstellen des Repos auf öffentlich: Git-Historie auf Zugangsdaten, Tokens und private Hostnamen prüfen und eine `.env.example` statt echter `.env`-Dateien einchecken.

### Tools installieren (Ubuntu 24.04)

Diese Befehle führst du einmalig im Terminal aus, in dieser Reihenfolge.

**1. Systempakete.** `build-essential` bringt den C-Compiler und Linker mit, den Rust braucht, `libssl-dev` und `pkg-config` werden für die Passkey-Bibliothek gebraucht.

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev git curl
```

**2. Rust über rustup.** Das ist der offizielle Weg laut [rust-lang.org](https://rust-lang.org/tools/install). Im Dialog wählst du die Standardinstallation (Option 1).

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Danach ein neues Terminal öffnen oder `. "$HOME/.cargo/env"` ausführen und prüfen:

```bash
rustc --version
cargo --version
cargo fmt --version
cargo clippy --version
```

Fehlt `cargo fmt` oder `cargo clippy`, hilft `rustup component add rustfmt clippy`. Später aktualisierst du Rust mit `rustup update`.

**3. Docker mit Compose-Plugin.** Ubuntu 24.04 (Noble) wird laut [Docker-Doku](https://docs.docker.com/engine/install/ubuntu/) unterstützt. Nimm Docker's eigenes Repository und nicht das Ubuntu-Paket `docker.io`.

```bash
sudo apt update
sudo apt install ca-certificates curl
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
sudo chmod a+r /etc/apt/keyrings/docker.asc

sudo tee /etc/apt/sources.list.d/docker.sources <<EOF
Types: deb
URIs: https://download.docker.com/linux/ubuntu
Suites: $(. /etc/os-release && echo "${UBUNTU_CODENAME:-$VERSION_CODENAME}")
Components: stable
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/docker.asc
EOF

sudo apt update
sudo apt install docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
sudo docker run hello-world
```

Ohne weitere Schritte braucht Docker `sudo`. Willst du ohne `sudo` arbeiten, folge der Docker-Seite [Post-installation steps for Linux](https://docs.docker.com/engine/install/linux-postinstall/). Beachte, dass Mitglieder der Gruppe `docker` praktisch Root-Rechte haben.

**4. Optional: sqlx-cli.** Brauchst du erst, wenn Claude Code Offline-Daten für Datenbankabfragen erzeugt. Claude Code sagt dir dann Bescheid.

```bash
cargo install sqlx-cli --no-default-features --features rustls,sqlite
```

Die zusätzlichen Systembibliotheken für den Tauri-Client (WebKitGTK und weitere) kommen erst mit Schritt C0, dafür gibt es dann eine eigene Liste.

## CLAUDE.md für das Repo

Diesen Text legst du unverändert als `CLAUDE.md` ins Repo-Root. Er ist auf Englisch, weil das Repo öffentlich wird, und legt Sprache, Befehle und Sicherheitsregeln für jeden Schritt fest.

```markdown
# SCRN

SCRN stands for Snap · Crop · Redact · Note, the workflow of the client. Self-hosted screenshot sharing: a Rust server (axum, SQLite) running in Docker and a Tauri desktop client. License: MIT.

## Language rules

- All code comments, identifiers, commit messages, docs, log messages and API error messages are written in English.
- The web UI is bilingual (German is the default, English is the second language). UI strings live in translation catalogs under `crates/scrn-server/locales`, never hard-coded in templates.

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
```

## Repo-Struktur und Datenmodell

Ein Cargo-Workspace trennt Server, geteilte API-Typen und später den Client. Die Daten liegen in sechs Tabellen.

```text
scrn/
├── CLAUDE.md
├── LICENSE                  MIT
├── README.md
├── Cargo.toml               workspace root
├── Dockerfile
├── compose.yaml
├── .env.example
├── .github/workflows/       CI and image publishing
├── crates/
│   ├── scrn-proto/          shared API types
│   └── scrn-server/
│       ├── migrations/
│       ├── locales/         de + en catalogs
│       ├── templates/
│       ├── static/          css, htmx, small js
│       └── src/
├── client/                  Tauri app (later)
└── docs/
```

| Tabelle | Wichtige Spalten | Zweck |
| --- | --- | --- |
| users | id, username, password\_hash, role (admin/user), language, theme, default\_expiry\_days, quota\_bytes, disabled\_at | Konten und persönliche Einstellungen |
| invites | id, token\_hash, created\_by, expires\_at, used\_by, used\_at | Einladungen des Admins, einmal nutzbar |
| sessions | id\_hash, user\_id, created\_at, last\_seen\_at, expires\_at | Web-Login |
| api\_tokens | id, user\_id, name, token\_hash, created\_at, last\_used\_at, revoked\_at | Tokens für den Client |
| screenshots | id (UUID), user\_id, visibility (private/public), short\_id (nur wenn öffentlich, eindeutig), width, height, bytes, thumb\_bytes, sha256, created\_at, expires\_at | Metadaten, Datei liegt unter `/data/files/<id>.png` |
| passkeys | id, user\_id, name, credential, created\_at, last\_used\_at | WebAuthn-Zugangsdaten |

Regeln zum Kurzlink: Nur öffentliche Bilder haben eine `short_id`. Wird ein Bild privat gemacht, wird die `short_id` gelöscht und der Link ist tot. Wird es wieder öffentlich, entsteht eine neue ID. Läuft ein Link ab, wird das Bild privat und bleibt in der Galerie, bis du es löscht.

Regeln zum Speicherkontingent: `users.quota_bytes` leer bedeutet Serverstandard (`SCRN_DEFAULT_QUOTA_MB`, Vorschlag 1024), `0` bedeutet unbegrenzt. Der Verbrauch ist die Summe aus `bytes` und `thumb_bytes` aller Bilder des Users. Der Admin ändert den Wert jederzeit in der Benutzerverwaltung, die Änderung gilt sofort. Wird das Kontingent unter den Verbrauch gesenkt, wird nichts gelöscht, nur neue Uploads werden mit HTTP 413 abgelehnt. Löschen gibt Platz sofort wieder frei.

## Server-Schritte S0 bis S4: Fundament, Login, Tokens

Nach diesen fünf Schritten läuft der Server im Container, du kannst dich anmelden, Leute einladen und Tokens erzeugen. Jeder Schritt hat Ziel, Prompt zum Einfügen in Claude Code und eine Abnahme.

### S0 – Repo-Bootstrap

**Ziel:** Workspace, Lizenz, CI. Vorher exportierst du dieses Dokument als Markdown und legst es als `docs/PLAN.md` ins Repo, die Prompts verweisen darauf.

```text
Read CLAUDE.md and docs/PLAN.md. Set up the repository: a Cargo workspace with the crates scrn-proto and scrn-server (empty but compiling), an MIT LICENSE (ask me for the copyright holder name, use the current year), an English README.md (project description, the meaning of the name SCRN = Snap, Crop, Redact, Note, status "early development", license), a .gitignore (Rust, .env, data/), rustfmt.toml, and a GitHub Actions workflow .github/workflows/ci.yml running cargo fmt --check, cargo clippy with -D warnings and cargo test. Write all comments in English. Commit as "chore: bootstrap workspace".
```

**Abnahme:**

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` und `cargo test --workspace` laufen ohne Fehler.
- Nach dem ersten Push ist die CI-Pipeline auf GitHub grün.

### S1 – Server-Grundgerüst und Docker

**Ziel:** Ein Container, der startet, loggt und auf `/healthz` antwortet.

```text
Implement the server skeleton in scrn-server: an axum server, configuration via environment variables (SCRN_BASE_URL, SCRN_BIND, SCRN_DATA_DIR, SCRN_COOKIE_SECURE, SCRN_TRUSTED_PROXIES), structured logging with tracing, GET /healthz, graceful shutdown, and SQLite via sqlx with automatic migrations on startup (WAL mode). Add a multi-stage Dockerfile (Rust builder, slim Debian runtime with libssl, non-root user, HEALTHCHECK) and a compose.yaml with one service, a ./data:/data volume, a healthcheck, restart: unless-stopped, the port bound to 127.0.0.1 by default and a read-only root filesystem where possible. Extend .env.example. Add an integration test for /healthz. Docker builds must not need a running database: either use runtime-checked sqlx queries, or commit the sqlx offline data (the .sqlx directory created with cargo sqlx prepare --workspace) and set SQLX_OFFLINE=true in the Dockerfile.
```

**Abnahme:**

- `docker compose up --build` startet, `curl http://127.0.0.1:8080/healthz` liefert 200.
- `docker compose exec` zeigt, dass der Prozess nicht als root läuft.
- Nach `docker compose down` und erneutem Start sind die Daten im Volume noch da.

### S2 – Datenmodell und Migrationen

**Ziel:** Alle sechs Tabellen mit Repository-Schicht, noch ohne HTTP-Routen.

```text
Create sqlx migrations for users, invites, sessions, api_tokens, screenshots and passkeys as described in the section "Repo-Struktur und Datenmodell" of docs/PLAN.md. Use foreign keys with ON DELETE CASCADE, a unique partial index on screenshots.short_id (where not null), indexes on user_id and expires_at, and a CHECK constraint that short_id may only be set when visibility is 'public'. Implement a repository layer (one module per table) with typed functions and tests against a temporary SQLite database. No HTTP routes yet.
```

**Abnahme:**

- Die Tests decken Anlegen, Lesen, Löschen und die Constraints ab, insbesondere `short_id` bei privaten Bildern.
- Beim Container-Start laufen die Migrationen automatisch durch.

### S3 – Setup, Login und Einladungen

**Ziel:** Admin anlegen, einloggen, Leute per Einladungslink registrieren.

```text
Implement authentication. On first start with no users, the server prints a one-time setup token to the log only. The page /setup creates the admin account with that token (username plus password of at least 12 characters) and is locked afterwards. Implement login and logout with argon2id, random session IDs (store only the hash), cookies HttpOnly, Secure (configurable), SameSite=Lax, CSRF tokens on all POST forms, and rate limiting on login per IP and per username with identical responses for unknown users. The admin can create invitations (link with expiry, single use) and users can register only via /invite/<token>. The admin can disable users. Keep the HTML plain and unstyled, styling comes later. Tests: setup flow, login, wrong password, single-use invitation, disabled user cannot log in.
```

**Abnahme:**

- Manueller Durchlauf: Setup-Token aus dem Log holen, Admin anlegen, Einladung erzeugen, zweiten User registrieren.
- Ein zweiter Aufruf von `/setup` wird abgewiesen, und ein benutzter Einladungslink ebenso.

### S4 – API-Tokens

**Ziel:** Tokens für den Client, mehrere pro User.

```text
Implement API tokens. A logged-in user can create named tokens (format scrn_ followed by 32 random bytes encoded as base62). The token is shown exactly once, the database stores only its SHA-256 hash. List tokens with name, created and last used, and allow revoking. Add Bearer authentication middleware for /api/v1/* that updates last_used_at (throttled). Add GET /api/v1/me returning the username and server version. Tests: valid, invalid and revoked token, and a token of a disabled user is rejected.
```

**Abnahme:**

- `curl -H "Authorization: Bearer <token>" https://…/api/v1/me` liefert den Usernamen, nach dem Widerrufen 401.
- Das Token taucht weder in der Datenbank im Klartext noch in den Logs auf.

## Server-Schritte S5 bis S8: Upload, Kurzlinks, Web-UI, Ablauf

Nach diesen Schritten ist der Server funktional komplett: Der Client kann hochladen, öffentliche Bilder sind über den Kurzlink erreichbar, die Galerie zeigt alles, und abgelaufene Links werden deaktiviert.

### S5 – Upload-API

**Ziel:** Ein PNG hochladen und Kurzlink oder ID zurückbekommen.

```text
Implement the upload API under /api/v1 with Bearer auth. POST /api/v1/screenshots accepts a raw PNG body (Content-Type image/png) with query parameters visibility=public|private (default private) and optional expires_in_days (public only; if absent use the user's default_expiry_days; 0 means never). Validate by decoding with limits (max body 20 MiB, max 100 megapixels), re-encode the image to strip all metadata, store it under /data/files/<uuid>.png, generate a thumbnail (max 480 px wide) and write the database row. For public images generate an 8-character base62 short_id from a CSPRNG, retry on collision and never produce a value that collides with a reserved route prefix. Response JSON: id, visibility, short_url (public only), expires_at, width, height. Also add GET /api/v1/screenshots?limit=N (newest first, includes short_url for public ones; the client uses limit=1 to re-copy the last link) and DELETE /api/v1/screenshots/{id}. Add per-user storage quotas. users.quota_bytes is NULL for the server default (SCRN_DEFAULT_QUOTA_MB, default 1024) and 0 for unlimited. Usage is the sum of bytes and thumb_bytes of the user's screenshots. Check the quota inside the same transaction that stores the row so concurrent uploads cannot exceed it, and reject an upload that would exceed it with HTTP 413 and the error code quota_exceeded, including the used and limit values in the response body. Extend GET /api/v1/me with used_bytes and quota_bytes (the effective value). Lowering a quota below the current usage never deletes anything, it only blocks new uploads, and deleting screenshots frees quota immediately. Add repository functions to read usage and to set a user's quota so the admin UI in S7a can use them. Put the request/response types in scrn-proto. Tests: valid upload, non-PNG, oversized image, metadata stripped, quota exceeded, other user's image cannot be deleted.
```

**Abnahme:**

- `curl -X POST -H "Authorization: Bearer $TOKEN" -H "Content-Type: image/png" --data-binary @test.png "http://127.0.0.1:8080/api/v1/screenshots?visibility=public"` liefert JSON mit `short_url`.
- Eine Textdatei mit der Endung `.png` wird mit 4xx abgelehnt. Ein Upload über dem Kontingent wird mit 413 und dem Code quota\_exceeded abgelehnt.

### S6 – Öffentliche Auslieferung

**Ziel:** `https://scrn.dev/<id>` zeigt das Bild, aber nur für öffentliche, nicht abgelaufene Einträge.

```text
Implement public serving: GET /{short_id} (and /{short_id}.png) returns the PNG only if the screenshot is public and not expired, otherwise an identical 404 for unknown, private and expired IDs. Set Content-Type image/png, X-Content-Type-Options nosniff, Content-Security-Policy default-src 'none', Content-Disposition inline and sensible Cache-Control. Make sure fixed routes (/api, /app, /static, /healthz, /setup, /invite, /login and so on) take precedence and that the short_id generator can never produce those names. Add per-IP rate limiting for this route, using the client IP from X-Forwarded-For only for trusted proxies. Tests: private image is never served, expired image returns 404, reserved names, headers.
```

**Abnahme:**

- Der Kurzlink aus S5 öffnet im Browser das Bild, ein privates Bild liefert dieselbe 404-Seite wie eine erfundene ID.

### S7a – Oberfläche, Sprachen, Dark und Light

**Ziel:** Alle bisherigen Seiten bekommen ein einheitliches Design mit Deutsch/Englisch und Dark/Light.

```text
Build the web UI foundation with server-rendered templates (askama or maud, your choice, justify it) and htmx; no SPA and no heavy JS framework. Use one CSS file with CSS variables for two themes, dark is the default. Set data-theme on the html element server-side to avoid a flash of the wrong theme. Store the theme and the language per user (users.theme, users.language) and, for logged-out pages, in a cookie. Language resolution: user setting, then cookie, then Accept-Language, then default German. Create translation catalogs de and en under crates/scrn-server/locales, no hard-coded UI strings in templates, and add a test that fails when a key exists in one catalog but not the other. Restyle the existing pages (setup, login, invite registration) and add a settings page (language, theme, default expiry for public links: never, 1, 7, 30 days or custom; API tokens, own storage usage as a progress bar) and an admin page (invitations, users, per-user storage usage and an editable quota where an empty value means the server default and 0 means unlimited; changes apply immediately). The layout must be responsive and keyboard accessible with sufficient contrast in both themes.
```

**Abnahme:**

- Beim ersten Aufruf erscheint die Seite auf Deutsch und im Dark-Mode, beides ist in den Einstellungen umschaltbar und bleibt nach dem Neuladen erhalten.
- Der Katalog-Test schlägt fehl, wenn du in `en` einen Schlüssel löschst.

### S7b – Galerie

**Ziel:** Alle Screenshots ansehen, den Kurzlink kopieren, Sichtbarkeit und Ablauf ändern, löschen.

```text
Build the gallery at /app: a responsive grid of thumbnails with lazy loading, htmx pagination, and a filter (all, public, private). Each public card shows its short URL with a copy button (Clipboard API with a select-text fallback), private cards show no link. The detail view shows the full image, size, creation date, expiry and short URL (public only), and lets the owner switch visibility (private to public creates a new short_id, public to private deletes the short_id and kills the link), change the expiry of a public link (never, 1, 7, 30 days, custom date) and delete the screenshot after a confirmation. Deleting removes the database row, the file and the thumbnail. All strings go through the translation catalogs. Tests: visibility switch semantics, only the owner can see or change an image, delete removes the files.
```

**Abnahme:**

- Ein öffentlicher Eintrag zeigt seinen Kurzlink samt Kopier-Button, ein privater Eintrag keinen.
- Nach „öffentlich → privat“ liefert der alte Kurzlink 404.

### S8 – Ablauf von Links

**Ziel:** Abgelaufene öffentliche Links werden automatisch deaktiviert.

```text
Implement link expiry. A background task (tokio) runs every 5 minutes and turns every public screenshot whose expires_at has passed into a private one by deleting its short_id and clearing expires_at; the image stays in the gallery. The serving route from S6 also checks expires_at on every request. Changing a user's default expiry only affects new uploads. Make the clock injectable for tests. Tests: expiry job, request-time check, per-image override, expiry 'never'.
```

**Abnahme:**

- Ein Bild mit Ablauf in einer Minute liefert danach 404 am Kurzlink und erscheint in der Galerie als privat.

## Server-Schritte S9 bis S11: Passkeys, Härtung, Release

Zum Schluss kommen Passkeys, Absicherung für den Betrieb im Internet hinter Nginx Proxy Manager und ein Release-Prozess, mit dem das Repo öffentlich gehen kann.

### S9 – Passkeys

**Ziel:** Anmeldung mit Passkey zusätzlich zum Passwort.

```text
Add passkey (WebAuthn) support using the webauthn-rs crate. Derive the relying party ID and origin from SCRN_BASE_URL (http://localhost must work for local development). In the settings page a logged-in user can register a passkey with a name, list, rename and delete passkeys. The login page offers "Sign in with passkey" (username first, then the passkey ceremony; add usernameless login only if the library supports it cleanly). Keep the password login as a fallback and refuse to delete the last remaining login method. Keep ceremony state server-side with a short TTL. Add translations for all new strings in both catalogs. Tests for the state handling and the database layer, and document how to test manually in a browser.
```

**Abnahme:**

- Passkey in den Einstellungen anlegen, abmelden, mit dem Passkey wieder anmelden (Browser, Passwortmanager oder Windows Hello).
- Das letzte Anmeldeverfahren lässt sich nicht löschen.

### S10 – Härtung und Betrieb hinter NPM

**Ziel:** Sicherer Betrieb im Internet mit dokumentierter NPM-Konfiguration und Backup.

```text
Harden the server for internet exposure. Add security headers on all HTML responses (strict Content-Security-Policy without inline scripts, frame-ancestors 'none', Referrer-Policy, Permissions-Policy, X-Content-Type-Options, HSTS when SCRN_BASE_URL is https), request timeouts and body size limits, and make sure no handler can panic on malformed input. Review the rate limits for login, registration, token use, upload and public serving. Write a Deployment section in the English README: compose usage, all environment variables, SCRN_TRUSTED_PROXIES with the Nginx Proxy Manager address, and the NPM proxy host settings (forward to the server port, Force SSL, HTTP/2, HSTS, Block Common Exploits, and in the Advanced tab client_max_body_size 25m;). Add scripts/backup.sh which creates a consistent SQLite backup plus an archive of /data/files, and document the restore. Write tests for the headers and the trusted proxy logic.
```

**Abnahme:**

- `curl -I https://deine-domain/` zeigt die Header, ein Upload von rund 20 MiB geht durch NPM durch.
- Backup erzeugen, Container mit leerem Volume starten, Backup zurückspielen: Galerie und Kurzlinks funktionieren wieder.

### S11 – Release und Öffentlichmachen

**Ziel:** Fertiges Image auf GHCR, Repo bereit für die Veröffentlichung unter MIT.

```text
Prepare the first release. Add a GitHub Actions workflow that builds the Docker image and publishes it to ghcr.io/<owner>/scrn (lowercase) on version tags (v*) using the built-in GITHUB_TOKEN with the permissions packages: write and contents: read (no personal access token), the actions docker/login-action, docker/metadata-action (tags: the version and latest) and docker/build-push-action, and the OCI label org.opencontainers.image.source so the package is linked to this repository, and update compose.yaml so an image reference can be used instead of a local build. Add dependency checks to CI (cargo audit or cargo deny) and Dependabot for cargo, docker and github-actions. Add SECURITY.md (how to report vulnerabilities), CONTRIBUTING.md, a CHANGELOG.md and issue templates. Review the repository for secrets, private hostnames and personal data before it becomes public, and list what you checked.
```

**Abnahme:**

- Ein Tag `v0.1.0` erzeugt ein Image, das du mit `docker compose pull` auf deinem Server starten kannst.
- Die Prüfliste von Claude Code ist durchgegangen, danach stellst du das Repo auf öffentlich.

### GHCR in Kurzform

ghcr.io ist die Container-Registry von GitHub. Dein Docker-Image liegt dort neben dem Code und gehört deinem Account, und dein Server holt es sich von dort.

1. **Anmeldung im Workflow:** Der Workflow nutzt den automatisch vorhandenen `GITHUB_TOKEN` mit der Berechtigung `packages: write`. Ein Personal Access Token brauchst du dafür nicht.
2. **Name:** `ghcr.io/<github-benutzer>/scrn`, komplett kleingeschrieben. Version `0.1.0` entsteht durch den Git-Tag `v0.1.0`.
3. **Zuerst privat:** Ein neues Paket ist beim ersten Veröffentlichen privat, auch wenn das Repo öffentlich ist. Nach dem ersten Release öffnest du auf GitHub dein Profil, dann Packages, dann `scrn`, dann Package settings und unter Danger Zone „Change visibility“ auf Public. Ein öffentliches Paket lässt sich laut GitHub-Doku nicht wieder privat stellen.
4. **Auf dem Server:** Ist das Paket öffentlich, trägt die `compose.yaml` `image: ghcr.io/<github-benutzer>/scrn:0.1.0`, und ein Update ist `docker compose pull` gefolgt von `docker compose up -d`, ohne Anmeldung.
5. **Falls das Paket privat bleibt:** Nach meinem Kenntnisstand (nicht in den Quellen geprüft) braucht der Server vorher `docker login ghcr.io` mit einem Personal Access Token (classic), das die Berechtigung `read:packages` hat. Einfacher ist, das Paket öffentlich zu machen.

## Client-Überblick

Der Client folgt nach dem Server in fünf Schritten, und ein Spike auf deinem Linux-Rechner klärt zuerst die größten Risiken. Die Prompts für C0 bis C4 schreibe ich im selben Format, sobald der Server steht.

1. **C0 – Spike (Ubuntu 24.04, GNOME 46, Wayland):** Tauri-App mit Tray-Icon und Menüeintrag „Screenshot aufnehmen“, Befehl `scrn capture`, Aufnahme über das Screenshot-Portal, Vollbild-Overlay mit Bereichsauswahl. Geprüft werden Skalierung und Multi-Monitor, und ob Tray-Klicks überhaupt ankommen. Ergebnis: Entscheidung, ob Tauri trägt.
2. **C1 – Linux-MVP:** Editor mit Pfeil in wählbarer Farbe, gelber Markierung und Blur, danach „Verwerfen“ oder „Speichern und hochladen“. Hochgeladen wird nur das fertig zusammengesetzte Bild. Einstellungsfenster über das Tray (Server-URL, Token im Schlüsselbund des Systems, Standard-Sichtbarkeit und -Ablauf, Autostart). Pakete als `.deb` und AppImage.
3. **C2 – Windows 11:** Aufnahme über die xcap-Bibliothek, globaler Hotkey (Print Screen), Installer und Autostart.
4. **C3 – Weitere Linux-Umgebungen:** KDE Plasma sowie Sway und Hyprland über das wlr-Portal. Hotkey über das GlobalShortcuts-Portal, sobald GNOME 48 oder neuer vorhanden ist.
5. **C4 – macOS:** Berechtigung für die Bildschirmaufnahme, Signierung und dmg. Erst, wenn ein Gerät zum Testen da ist.

**Letzten Link wieder kopieren:** Nach jedem Upload kopiert der Client den Kurzlink in die Zwischenablage und merkt sich ihn in einem lokalen Verlauf (die letzten 10 Einträge). Das Tray-Menü bietet „Letzten Link kopieren“ und eine Liste der letzten Links, und der Befehl `scrn copy-last` tut dasselbe, etwa für ein Tastenkürzel. Ist der lokale Verlauf leer, fragt der Client `GET /api/v1/screenshots?limit=1` beim Server ab. Private Bilder haben keinen Kurzlink. Dort öffnet das Menü stattdessen die Galerie.

## Arbeitsweise mit Claude Code

Ein Schritt entspricht einem Branch und einer frischen Claude-Code-Sitzung, denn `CLAUDE.md` und `docs/PLAN.md` liefern den gesamten Kontext.

1. Branch anlegen, z. B. `git switch -c s3-auth`.
2. Claude Code im Repo-Ordner starten, im Plan-Modus den Prompt des Schritts einfügen und den Plan lesen, bevor Code entsteht. Abweichungen vom Plan jetzt korrigieren.
3. Umsetzen lassen, danach die Abnahme des Schritts selbst durchführen.
4. Pull Request öffnen, CI abwarten, mergen.

**Fertig ist ein Schritt, wenn:** `fmt`, `clippy` und Tests grün sind, die Abnahme erfüllt ist, README und `.env.example` zu neuen Einstellungen passen, die Commits dem Schema aus `CLAUDE.md` folgen und die CI auf GitHub grün ist.

Am Ende jedes Prompts hilft dieser Zusatz:

```text
Before you finish: run fmt, clippy and the tests, update README and .env.example if configuration changed, and list every assumption you made and every open question.
```

Nach S3, S5 und S10 lohnt sich ein eigener Sicherheitsdurchgang in einer neuen Sitzung:

```text
Review the diff of this branch against main for security problems. Check it against the security rules in CLAUDE.md, especially authentication, session handling, token storage, upload validation and the separation of private and public screenshots. Report findings with file and line, do not change code yet.
```

Wenn du eine Entscheidung dieses Plans änderst, passe zuerst `docs/PLAN.md` an. Dann gilt sie auch für alle folgenden Sitzungen.

## Annahmen, offene Punkte, Quellen

Ich habe neun Dinge selbst entschieden, die du nicht ausdrücklich vorgegeben hast. Sag Bescheid oder ändere sie hier, bevor der jeweilige Schritt startet.

**Annahmen**

1. Wenn ein öffentlicher Link abläuft, wird das Bild privat und bleibt in der Galerie. Es wird nicht gelöscht.
2. Die Sichtbarkeit lässt sich nachträglich ändern. Öffentlich zu privat macht den alten Kurzlink ungültig, privat zu öffentlich erzeugt eine neue ID.
3. Kurzlinks liegen direkt unter der Domain (`scrn.dev/<id>`) und haben 8 Zeichen Base62, das sind rund 2,2 · 10^14 Möglichkeiten.
4. Uploads sind nur PNG, höchstens 20 MiB und 100 Megapixel, Metadaten werden entfernt.
5. Den ersten Admin legst du über einen einmaligen Setup-Token an, der beim ersten Start im Log steht.
6. Admins sehen die Screenshots anderer Nutzer nicht. Sie verwalten nur Einladungen und Konten.
7. Ein Token darf hochladen, die eigene Liste lesen und eigene Bilder löschen. Feinere Rechte gibt es zunächst nicht.
8. Die Template-Engine (askama oder maud) wählt Claude Code und begründet es.
9. SQLite reicht für Freunde, Familie und Kollegen. Ein späterer Wechsel auf PostgreSQL wäre möglich, aber Aufwand.

**Offene Punkte**

- Name für den MIT-Copyright-Hinweis (Claude Code fragt in S0).
- GitHub-Benutzername oder -Organisation für das Image auf ghcr.io.
- Standardwert des Speicherkontingents (Vorschlag: 1024 MB pro Nutzer, einstellbar mit SCRN\_DEFAULT\_QUOTA\_MB, pro Nutzer vom Admin änderbar).

**Quellen** (Suchergebnisse vom 5. November 2026, die Seiten wurden nicht einzeln geöffnet)

- [xcap, Bibliothek für Bildschirmaufnahme (lib.rs)](https://lib.rs/crates/xcap)
- [GlobalShortcuts-Portal, Unterstützung ab GNOME 48 (PyPI)](https://pypi.org/project/global-shortcut-portal/0.2.0/)
- [Tauri: globale Shortcuts unter Wayland deaktiviert (tao)](https://github.com/tauri-apps/tao/commit/27ab6f4dcef19c052b0434873e34b54447b70860)
- [Screenshot-Portal, interaktiv und nicht-interaktiv (xdg-desktop-portal)](https://github.com/flatpak/xdg-desktop-portal/issues/1065)
- [xdg-desktop-portal-wlr (Screenshot und ScreenCast)](https://github.com/emersion/xdg-desktop-portal-wlr)
- [Tray-Icons unter GNOME (Aikido-Doku)](https://help.aikido.dev/aikido-device-protection/deploying-aikido-endpoint/device-protection-mdm-guides/linux/install-device-protection-for-linux/tray-icon-support-on-linux.md)
- [webauthn-rs (openapps.pro)](https://openapps.pro/packages/webauthn-rs)
- [Ubuntu 26.04 mit GNOME 50 (linuxconfig)](https://linuxconfig.org/ubuntu-26-04-release-date-and-new-features-in-resolute-raccoon)

**Weitere Quellen** (abgerufen oder gesucht am 5. November 2026)

- [Rust installieren (rust-lang.org)](https://rust-lang.org/tools/install)
- [Docker Engine auf Ubuntu installieren](https://docs.docker.com/engine/install/ubuntu/)
- [GitHub: Docker-Images mit Actions veröffentlichen (Enterprise-Server-Version der Doku)](https://docs.github.com/en/enterprise-server@3.15/actions/use-cases-and-examples/publishing-packages/publishing-docker-images)
- [GitHub: Sichtbarkeit und Zugriff von Paketen (ältere Enterprise-Server-Version der Doku)](https://docs.github.com/en/enterprise-server@3.1/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility)
- [Beispiel-PR: GHCR-Pakete sind standardmäßig privat, OCI-Labels verknüpfen sie mit dem Repo](https://github.com/RightNow-AI/openfang/pull/965)
- [sqlx-cli: Offline-Modus für Docker-Builds](https://docs.rs/crate/sqlx-cli/latest)
