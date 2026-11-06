# C0 spike: Tauri on Ubuntu 24.04 (GNOME 46, Wayland)

The spike in `client/` answers one question: does Tauri carry the desktop
client on Linux with Wayland? It is not the client yet: no editor, no
upload, no settings.

## What it does

- Tray icon with the menu *Screenshot aufnehmen*, the path of the report
  and *Beenden*.
- `scrn capture` takes a screenshot. If the tray app already runs, the
  command is handed over to it (single instance over D-Bus).
- The screenshot comes from the xdg-desktop-portal (`interactive=false`).
  GNOME stores it in the pictures folder; the spike reads it and removes
  the file at once.
- One fullscreen overlay per monitor shows the frozen screen. Wayland
  ignores window positions, so each overlay is put fullscreen on its
  monitor through GTK (`gtk_window_fullscreen_on_monitor`). Drag a
  rectangle on one monitor to save that part as
  `~/Pictures/SCRN/scrn-spike-<time>.png`; Esc cancels on all monitors.
- From the tray menu, the capture waits 400 ms so the closing menu is not
  in the screenshot.
- Every observation goes to `~/.local/state/scrn/spike.log`: session,
  monitors with position, size and scale, tray events, portal timing,
  window and image sizes, the saved selection.

## Build and run

```sh
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev librsvg2-dev libxdo-dev
cd client
cargo run              # tray icon
cargo run -- capture   # in a second terminal: capture via the command
```

## Checklist

| # | Check | Expected |
|---|-------|----------|
| 1 | Tray icon appears in the top bar | icon visible, menu opens on click |
| 2 | Left and right click on the icon | menu opens; report shows whether click events arrive |
| 3 | *Screenshot aufnehmen* | portal permission dialog on first use, then the overlay |
| 4 | Overlay on the main monitor | frozen screen, sharp, no offset to the real screen |
| 5 | Drag a rectangle | size label in image pixels, Esc cancels, release saves |
| 6 | Saved PNG | exactly the selected area, sharp |
| 7 | Second monitor | both monitors get an overlay, each shows its own part |
| 8 | `scrn capture` while the tray app runs | handed over, overlay opens |
| 9 | Fractional scaling (e.g. 125 %) | overlay and saved area still match |

## Results

### Round 1 (2026-11-06)

Machine: Ubuntu 24.04, GNOME, Wayland. Two monitors at scale 1:
HG342PCB 3440×1440 at (0, 0) and BenQ EW2420 1920×1080 at (3440, 0).

| # | Result |
|---|--------|
| 1 | ok: icon in the top bar, menu opens |
| 3 | ok: permission dialog on first use (3.3 s), later captures take about 0.6 s |
| 4 | ok: sharp, no visible offset |
| 5, 6 | ok: three selections saved, sizes match |
| 7 | **failed**: the portal image covers both monitors (5360×1440), but only the left monitor got an overlay; the right one kept running |
| – | GNOME flashes the screen on every capture. The closing tray menu was in the screenshot. |

Changes after round 1: one overlay per monitor, and the delay after the
tray menu. The flash comes from GNOME's portal backend with
`interactive=false` and cannot be turned off from the app; the
alternative is the ScreenCast portal (a single PipeWire frame), which is
a decision for C1.

### Round 2 (2026-11-06)

| # | Result |
|---|--------|
| 3 | ok: captures take about 0.6 s |
| 7 | ok: overlay 0 on HG342PCB shows (0, 0, 3440×1440), overlay 1 on BenQ EW2420 shows (3440, 0, 1920×1080); a selection on the right monitor was saved at x=3897, Esc closes both overlays |
| – | ok: the tray menu is no longer in the screenshot |
| 2 | no tray click events arrive (AppIndicator); the menu is the only way in, which is enough |
| 8, 9 | not tested yet: hand-over of `scrn capture`, fractional scaling |

## Conclusion

Tauri carries the client on GNOME with Wayland: tray menu, portal
capture, sharp per-monitor overlays and exact crops all work. Open for
C1: the GNOME flash (ScreenCast portal as an option), a selection across
monitors, fractional scaling and the global shortcut through
`scrn capture`.
