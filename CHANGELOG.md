# Changelog

All notable changes to this project are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- View counter for public links, shown in the gallery, on the detail page
  and as `views` in the API.
- Backup download in the administration: a tar archive with a consistent
  database snapshot and all screenshot files, streamed without storing a
  copy on the server.

### Changed

- Times in the web UI are shown in the browser's time zone (detected
  automatically, UTC without JavaScript). A chosen expiry date ends at
  local midnight.
- Screenshot files are stored per user in `files/<user id>/`. Files of the
  earlier flat layout are moved there at startup.

## [0.1.0] - unreleased

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
  filter, copy button and detail view, settings, administration.
- Security headers on every response, request timeouts and body limits.
- `scrn-server backup` and `scripts/backup.sh` for consistent backups.

[Unreleased]: https://github.com/andreaslink-de/scrn/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/andreaslink-de/scrn/releases/tag/v0.1.0
