# Changelog of the desktop client

The client has its own version and its own tags (`client-vX.Y.Z`). The
format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Windows 10 and 11: capture of all monitors with xcap, an overlay on
  every monitor, Print Screen (or another shortcut from the settings) as
  global shortcut, autostart through the Run key of the user, tray icon
  matching the taskbar theme, and an NSIS installer for the current user
  (no administrator rights).

## [0.1.0] - 2026-11-06

First release of the desktop client, for Linux with GNOME on Wayland
(tested on Ubuntu 24.04, GNOME 46).

### Added

- Tray icon with *Screenshot aufnehmen*, *Letzten Link kopieren*, the
  last ten links, *Galerie öffnen* and the settings.
- `scrn capture` and `scrn copy-last` for keyboard shortcuts; a second
  start hands the command to the running client.
- Capture through the screenshot portal and a frozen overlay on every
  monitor to select an area.
- Editor with arrows in six colors, a yellow highlighter and blur that
  removes the detail for good; undo, keyboard shortcuts.
- Upload to the SCRN server with an API token from the system keyring.
  Public uploads put their short link on the clipboard; without a server
  or after a failed upload the image is saved under `~/Pictures/SCRN`.
- Settings: server address, token with connection test, visibility and
  expiry of new uploads, a local copy, autostart.
- German and English, following the system language.
- Packages as `.deb` and AppImage.
