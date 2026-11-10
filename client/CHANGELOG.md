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
- KDE Plasma, Sway and Hyprland: screenshots through their portals, and
  through `grim` on wlroots compositors whose portal cannot take them.
  The README lists what each desktop needs and how to bind a shortcut.
- The version is shown in the tray menu (*Über SCRN*), at the bottom of
  the settings and by `scrn --version`, together with links to the
  project on GitHub and to scrn.dev.
- Keyboard shortcuts through the GlobalShortcuts portal where the desktop
  offers it (KDE Plasma, Hyprland, GNOME 48 or newer): *capture* with
  Print suggested, and *copy-last*.
- Windows: *Record* next to the shortcut field takes the next key
  combination, so the shortcut no longer has to be typed.

### Changed

- The permission window appears on GNOME only; it is the only desktop
  that keeps the screenshot permission in the permission store.

### Fixed

- Windows: no console window next to the tray app; closing it no longer
  quits the client.
- Saved images go to the pictures folder the system names, such as
  `~/Bilder` on a German Linux desktop or a Pictures folder moved into
  OneDrive on Windows, instead of always `~/Pictures`. Images saved
  before stay where they are.
- A selection across several monitors shows on every monitor it covers,
  not only on the one where the drag started; monitors outside it are
  dimmed.

## [0.1.1] - 2026-11-07

### Fixed

- Screenshots work when the client is started from the menu after
  installing the package. GNOME refused them because the app had no
  stored screenshot permission and its own question had no window to
  appear on. On the first refusal the client now asks in its own window
  and stores the permission after *Allow*.
- A failed screenshot shows a notification instead of doing nothing.

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
