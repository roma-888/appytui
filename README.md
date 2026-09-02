# appytui

Apple Music in your terminal, with a real audio spectrum visualizer.

- Browse Songs, Albums, Artists, Playlists; fuzzy search; queue view
- Play, pause, skip, seek, volume, shuffle, repeat via Music.app
- cava-style visualizer driven by a Core Audio tap on Music.app (no cava needed)
- Album art rendered in half-block cells
- Themes: cava theme files work unchanged

## Requirements

macOS 14.2+, Rust 1.85+, Music.app.

## Install

    cargo install --path .

## Permissions

On first run macOS asks to allow your terminal to control Music.app (Automation) and
to record system audio (Screen & System Audio Recording). Both attach to the terminal
app, not to appytui. If you decline audio recording the visualizer runs simulated.

## Keys

| Key | Action |
|---|---|
| 1–6 / Tab | switch tab |
| j/k ↓/↑, g/G, Ctrl-d/u | move |
| Enter / Backspace | play or open / back |
| Space, n, p | pause, next, previous |
| ← / → | seek 5 s |
| + / - | volume |
| s / r | shuffle / repeat |
| / then Esc | filter / clear |
| v / V / w | visualizer on-off / orientation / waveform |
| ? / q | help / quit |

## Config

`~/.config/appytui/config.toml` is created on first run with every option commented.
Themes resolve from `~/.config/appytui/themes/`, then `~/.config/cava/themes/`, then the
built-ins (`catppuccin-mocha`, `catppuccin-mocha-h`, `solarized_dark`, `tricolor`, `terminal`).

Flags: `--theme NAME`, `--config PATH`, `--no-viz`, `--no-art`.

## Known limitations

- AirPlay output: the tap listens to the Mac's default output device, so streaming to
  AirPlay speakers shows a quiet visualizer.
- Library only: no Apple Music catalog search.
