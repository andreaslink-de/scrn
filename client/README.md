# SCRN desktop client

In development. It runs on Ubuntu 24.04 with GNOME on Wayland; other
desktops follow later (see `docs/PLAN.md`).

## Install

Packages for Linux come with each client release (`client-v*` tags):

- **Debian/Ubuntu:** `sudo apt install ./SCRN_<version>_amd64.deb`. It
  installs `scrn` to `/usr/bin`, a menu entry and the icon.
- **AppImage:** `chmod +x SCRN_<version>_amd64.AppImage`, then start it.
  Autostart (in the settings) points to the AppImage file, so keep it in
  one place, e.g. `~/Applications`.

Start SCRN from the app menu; it lives in the tray. For a keyboard
shortcut, add `scrn capture` (and `scrn copy-last`) under *Settings →
Keyboard → Custom Shortcuts*; with the AppImage use its full path
followed by `capture`.

## Build packages

```sh
cargo install tauri-cli --version "~2.12.4" --locked
cd client
cargo tauri build --bundles deb,appimage
```

The packages land in `target/release/bundle/deb` and
`target/release/bundle/appimage`.

## Build and run

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev
cd client
cargo run              # tray icon
cargo run -- capture   # take a screenshot (hands over to the running app)
cargo run -- copy-last # copy the link of the last upload
```

## Set up

1. Create an API token in the web UI under *Einstellungen → API-Tokens*.
2. Tray menu → *Einstellungen …*: enter the server address and the
   token, *Verbindung testen*, then *Speichern*. The token goes to the
   system keyring (GNOME Keyring), never into a file.
3. Choose the visibility of new uploads (private by default), the
   expiry of public links and whether the client starts on login.

Settings live in `~/.config/scrn/config.json`, the last ten uploads in
`~/.local/state/scrn/history.json`.

## Use

1. *Screenshot aufnehmen* in the tray menu, or `scrn capture`. The first
   capture asks for the screenshot permission.
2. Every monitor shows the frozen screen. Drag a rectangle; Esc cancels.
3. The editor opens with the selected area:

| Tool | Key | What it does |
|------|-----|--------------|
| Arrow | `1` | arrow in the chosen color |
| Highlight | `2` | translucent yellow rectangle |
| Blur | `3` | makes the area unreadable (downscaled, the detail is gone) |

   `Ctrl+Z` undoes the last shape. `Enter` or `Ctrl+S` uploads, Esc
   discards (press it twice once there are shapes).
4. The tray shows empty brackets while the upload runs. A public upload
   puts its short link on the clipboard; a private one has no link and
   shows up in the gallery. Without a server, or when the upload fails,
   the image is saved under `~/Pictures/SCRN` instead.

The tray menu has *Letzten Link kopieren* (also `scrn copy-last`, e.g.
for a keyboard shortcut) and *Letzte Links* with the last ten uploads:
public ones are copied, private ones open in the gallery.

The client log is `~/.local/state/scrn/client.log`. It records the
desktop, monitors and timings, never image data.
