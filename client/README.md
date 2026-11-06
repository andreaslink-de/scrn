# SCRN desktop client

In development. It runs on Ubuntu 24.04 with GNOME on Wayland; other
desktops follow later (see `docs/PLAN.md`).

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
