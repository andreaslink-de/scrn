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
- A fullscreen overlay shows the frozen screen. Drag a rectangle to save
  that part as `~/Pictures/SCRN/scrn-spike-<time>.png`; Esc cancels.
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
| 7 | Second monitor | which monitor gets the overlay, and does it show the right part |
| 8 | `scrn capture` while the tray app runs | handed over, overlay opens |
| 9 | Fractional scaling (e.g. 125 %) | overlay and saved area still match |

## Results

To be filled in after the test on the target machine.
