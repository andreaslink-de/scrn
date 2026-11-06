# Changelog

All notable changes to this project are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

[Unreleased]: https://github.com/andreaslink-de/scrn/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/andreaslink-de/scrn/releases/tag/v0.1.0
