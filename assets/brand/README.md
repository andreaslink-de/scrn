# SCRN brand assets

Made for scrn.dev. The SVGs here are the sources; the rasters live where
they are used (see below). Suffixes name the background an asset is
meant for: `-dark` / `on-dark` for dark backgrounds, `-light` /
`on-light` for light ones.

## Colors

| Use | Dark background | Light background |
|-----|-----------------|------------------|
| Mark green | `#34E58C` | `#0E9F58` |
| Text and center bar | `#F1FBF5` | `#0B1F16` |
| `.dev` suffix | `#7FA48F` | `#5E7A6B` |
| Tray green | `#3EE08A` | `#0A7A44` |
| App icon tile | gradient `#0F2A1E` → `#071510` | |

## Files

| File | What | Used in |
|------|------|---------|
| `svg/scrn-app-icon.svg` | app icon: dark tile, green brackets, bright center bar | source of the Tauri icons |
| `svg/scrn-app-icon-small.svg` | app icon simplified for small sizes (all bars green) | web favicon |
| `svg/scrn-logo-dark.svg`, `svg/scrn-logo-light.svg` | mark without tile | web UI header |
| `svg/scrn-lockup-dark.svg`, `svg/scrn-lockup-light.svg` | mark with "scrn.dev" | top of the README |
| `svg/scrn-tray-on-dark.svg`, `svg/scrn-tray-on-light.svg` | tray mark in one color | client tray (Linux, Windows) |
| `svg/scrn-tray-template.svg` | tray mark in black, colored by the system | client tray (macOS template) |
| `svg/scrn-tray-empty-*.svg` | brackets only, without the bars | spare, e.g. for a busy state |

Rasters:

- `client/icons/`: the Tauri set (PNG, `icon.ico`, `icon.icns`, Windows
  `Square*Logo.png`), and `client/icons/tray/` with the tray icons in
  16, 22, 24, 32 (`@2x`) and 64 px; the empty variant in `tray/empty/`.
- `crates/scrn-server/static/`: `favicon.svg`, `favicon.ico`,
  `apple-touch-icon.png` (180 px), `icon-512.png`, `logo-dark.svg`,
  `logo-light.svg`.

A mobile app can take its icons from the same SVGs: the app icon for the
launcher, the logo for in-app use.

License: MIT, like the rest of the repository.
