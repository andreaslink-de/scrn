# Changelog

All notable changes to this project are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Change the password under *Settings → Password*. Instead of the current
  password, a passkey works too: a passkey login or *Confirm with passkey*
  replaces it for 10 minutes. Other sessions of the account end.
- Password reset links: an admin creates one per user on the
  administration page. It is shown once, works once within 24 hours and
  ends all sessions of the account.
- `scrn-server reset-link <username>` prints a reset link on the command
  line, so an admin who lost their password gets back in
  (`docker compose exec scrn scrn-server reset-link admin`).
- After the setup and after registering through an invitation, the
  settings page suggests adding a passkey.

### Changed

- The setup token field is a plain text field marked as one-time code, so
  password managers no longer offer to store the token as the admin
  password.
- `reset` is a reserved short ID prefix.
- English is the default language when neither the account, a cookie nor
  the browser asks for one. Existing accounts keep their setting.
- The custom days field in the settings and the date field on a
  screenshot only show while the custom option is chosen.
- Short links in the gallery are no longer underlined; the account name
  in the header and the version in the footer stand apart from the
  links and the description next to them.

## [0.3.0] - 2026-11-06

### Added

- SCRN brand icons: new favicon (SVG and ICO, also at `/favicon.ico`),
  an icon for the home screen of phones (`apple-touch-icon`), a web app
  manifest and the logo in the header, matching the theme. The sources
  live in `assets/brand`.

### Changed

- The accent color is the brand green instead of blue (`#34e58c` in the
  dark theme, `#0a7a44` in the light one for enough contrast).
- Gallery cards show only the short ID of a public link instead of the
  cut-off URL; *Kopieren* still copies the whole URL.
- Copying links also works without HTTPS: when the browser offers no
  Clipboard API, the copy command is used before falling back to
  selecting the text.
- The page CSP allows `manifest-src 'self'` for the web app manifest.

## [0.2.0] - 2026-11-06

### Added

- The footer shows the server version and links to the source repository.
- `SCRN_TRUSTED_PROXIES` accepts host names, such as the container name of
  Nginx Proxy Manager; they are resolved at startup and every 30 seconds.

### Changed

- The default port is 8081 instead of 8080 (`SCRN_BIND`,
  `SCRN_PUBLISH_PORT`, the image's `EXPOSE`). Set `SCRN_BIND=0.0.0.0:8080`
  and `SCRN_PUBLISH_PORT=8080` to keep the old port.
- `compose.yaml` names the container `scrn`.

## [0.1.0] - 2026-11-06

First release of the SCRN server.

### Added

- Server in Rust (axum, SQLite) as a Docker image with a non-root user,
  read-only root file system and health check.
- First-start setup with a one-time token from the log; registration only
  through single-use invitations; administrators can disable accounts.
- Login with password (argon2id) or passkey (WebAuthn), sessions in
  HttpOnly `SameSite=Lax` cookies, CSRF protection on every form, rate
  limits for logins, registrations and API tokens.
- API tokens for the desktop client (shown once, stored as SHA-256 hash).
- Upload API for PNG screenshots: decoded with limits, re-encoded without
  metadata, thumbnails, per-user storage quotas enforced in the database
  transaction.
- Public short links (8 characters) with optional expiry, identical 404 for
  unknown, private and expired links; expired links become private
  automatically.
- Web UI in German and English with dark and light theme: gallery with
  filter, copy button and detail view, settings, administration. Times are
  shown in the browser's time zone.
- View counter for public links, shown in the gallery, on the detail page
  and as `views` in the API.
- Screenshot files stored per user in `files/<user id>/`.
- Security headers on every response, request timeouts and body limits.
- Consistent backups while the server keeps running: a download in the
  administration, `scripts/backup.sh` and `scrn-server backup`.

[Unreleased]: https://github.com/andreaslink-de/scrn/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/andreaslink-de/scrn/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/andreaslink-de/scrn/releases/tag/v0.1.0
