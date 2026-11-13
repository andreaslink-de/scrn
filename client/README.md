# SCRN desktop client

Runs on Linux (tested on Ubuntu 24.04 with GNOME on Wayland; KDE Plasma,
Sway and Hyprland are in testing, see [Linux desktops](#linux-desktops))
and on Windows 10/11. macOS follows (see `docs/PLAN.md`).

## Install

Packages for Linux come with each client release (`client-v*` tags):

- **Debian/Ubuntu:** `sudo apt install ./SCRN_<version>_amd64.deb`. It
  installs `scrn` to `/usr/bin`, a menu entry and the icon.
- **AppImage:** `chmod +x SCRN_<version>_amd64.AppImage`, then start it.
  Autostart (in the settings) points to the AppImage file, so keep it in
  one place, e.g. `~/Applications`.

- **Windows:** run `SCRN_<version>_x64-setup.exe`. It installs for the
  current user only, without administrator rights, and adds a Start menu
  entry. The installer is not signed yet, so SmartScreen asks once
  (*More info → Run anyway*).

Start SCRN from the app menu; it lives in the tray. For keyboard
shortcuts on Linux see [Keyboard shortcuts](#keyboard-shortcuts); with
the AppImage use its full path followed by `capture`. On Windows the client registers the shortcut
itself: Print Screen by default, changeable in the settings. Windows 11
opens its own screen capture on Print Screen; turn that off under
*Settings → Accessibility → Keyboard*.

## Linux desktops

The client takes screenshots through the screenshot portal
(xdg-desktop-portal) and needs a tray (StatusNotifierItem) and a Secret
Service keyring for the token.

| Desktop | Screenshot | Tray | Keyring |
|---------|------------|------|---------|
| GNOME | `xdg-desktop-portal-gnome`; asks once for the permission | AppIndicator extension (preinstalled on Ubuntu) | GNOME Keyring |
| KDE Plasma | `xdg-desktop-portal-kde` | built in | KWallet |
| Sway | `xdg-desktop-portal-wlr`, or `grim` | a bar with tray, e.g. Waybar's `tray` module | GNOME Keyring or KeePassXC |
| Hyprland | `grim` (its portal has no screenshots) | Waybar's `tray` module | GNOME Keyring or KeePassXC |

On wlroots compositors (Sway, Hyprland and similar) the client runs
`grim` when the portal cannot take the screenshot, so install `grim`
there.

### Keyboard shortcuts

Where the desktop offers the GlobalShortcuts portal, the client registers
two shortcuts on start: *capture* (Print suggested) and *copy-last* (no
key suggested). The desktop decides the keys:

- **KDE Plasma:** asks on the first start; change the keys under
  *System Settings → Keyboard → Shortcuts → SCRN*.
- **GNOME 48 or newer:** asks once in a dialog; change the keys under
  *Settings → Apps → SCRN*.
- **Hyprland:** binds nothing itself. `hyprctl globalshortcuts` lists
  the names, then bind one in `~/.config/hypr/hyprland.conf`, e.g.
  `bind = , Print, global, :capture` (the part before the colon is the app
  ID the portal saw, empty when started from a terminal or `exec`).

Without the portal (GNOME 46 and older, Sway), bind `scrn capture` and
`scrn copy-last` in the desktop's own settings:

- **GNOME:** *Settings → Keyboard → Custom Shortcuts*.
- **Sway** (`~/.config/sway/config`): `bindsym Print exec scrn capture`

## Build packages

```sh
cargo install tauri-cli --version "~2.12.4" --locked
cd client
cargo tauri build --bundles deb,appimage
```

The packages land in `target/release/bundle/deb` and
`target/release/bundle/appimage`. On Windows (with the Rust MSVC
toolchain), `cargo tauri build --bundles nsis` builds the installer in
`target/release/bundle/nsis`.

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
   system keyring (GNOME Keyring, KWallet or another Secret Service),
   never into a file.
3. Choose the visibility of new uploads (private by default), the
   expiry of public links and whether the client starts on login.

Settings live in `~/.config/scrn/config.json` (Windows:
`%APPDATA%\SCRN\config.json`), the last ten uploads and the log in
`~/.local/state/scrn/` (Windows: `%LOCALAPPDATA%\SCRN\`). The token is
in the GNOME Keyring or the Windows Credential Manager.

## Use

1. *Screenshot aufnehmen* in the tray menu, or `scrn capture`. On GNOME
   the first capture asks for the screenshot permission.
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
   the image is saved in the folder `SCRN` inside your pictures folder
   instead (e.g. `~/Bilder/SCRN` or `~/Pictures/SCRN`, as the system names
   it).

The tray menu has *Letzten Link kopieren* (also `scrn copy-last`, e.g.
for a keyboard shortcut) and *Letzte Links* with the last ten uploads:
public ones are copied, private ones open in the gallery.

*Über SCRN* in the tray menu shows the version and links to the project
on GitHub and to scrn.dev; the settings show the same at the bottom, and
`scrn --version` prints the version.

Once a day, and 30 seconds after the start, the client asks GitHub's
public API for the list of releases. When a newer client release is
published, the tray menu shows *Update auf … verfügbar* at the top, which
opens the release page, and a notification says so once. Nothing is
downloaded or installed. The request carries no token or user data, but
GitHub sees your IP address; *Täglich auf GitHub nach einer neuen Version
suchen* in the settings turns it off.

The client log is `client.log` next to the history. It records the
desktop, monitors and timings, never image data, tokens or links.
