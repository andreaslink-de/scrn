# SCRN desktop client

In development. It runs on Ubuntu 24.04 with GNOME on Wayland; other
desktops follow later (see `docs/PLAN.md`). Upload to the server is not
wired up yet: finished images are saved under `~/Pictures/SCRN`.

## Build and run

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev
cd client
cargo run              # tray icon
cargo run -- capture   # take a screenshot (hands over to the running app)
```

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

   `Ctrl+Z` undoes the last shape. `Enter` or `Ctrl+S` saves, Esc discards
   (press it twice once there are shapes).

The client log is `~/.local/state/scrn/client.log`. It records the
desktop, monitors and timings, never image data.
