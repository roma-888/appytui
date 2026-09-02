# appytui — Apple Music TUI: design

Date: 2026-09-02
Status: approved design, pending implementation plan

## 1. Goal

A keyboard-driven terminal client for Apple Music on macOS. It browses the
user's library, controls playback, and shows a real-time audio spectrum
visualizer (a cava reimplementation in Rust) next to the now-playing track.

## 2. Scope

In scope for v1:

- Browse Songs, Albums, Artists, and Playlists from the local Music.app library.
- Fuzzy search across title, artist, and album.
- Play a track in the context of the list it was chosen from; play/pause, next,
  previous, seek, volume, shuffle, repeat.
- A Queue view showing what will play next in the current context.
- Now Playing panel: album art, track metadata, progress, and the visualizer.
- Real audio spectrum visualizer driven by a Core Audio process tap and an FFT.
- Simulated visualizer fallback when audio capture is unavailable.
- cava-compatible visualizer settings (orientation, stereo/mono, bar geometry,
  monstercat/waves smoothing, gradients, waveform) and cava-compatible theme
  files, configured in a TOML config file.

Out of scope for v1:

- Apple Music catalog search or recommendations (MusicKit). Local library only.
- Mouse support.
- Lyrics.
- Editing the library or playlists.
- Non-macOS platforms.

## 3. Platform and stack

- macOS 14.2 or newer (Core Audio process taps). Developed on macOS 26.4.
- Rust 2021 edition, stable toolchain (1.96 available).
- Crates: `ratatui` 0.30, `crossterm` 0.29, `cidre` 0.24 (`core_audio` feature
  only, `macos_14_2`), `rustfft` 6, `serde` + `serde_json`, `image` (JPEG decode),
  `ureq` (album art HTTP), `nucleo-matcher` (fuzzy filter), `crossbeam-channel`,
  `toml` + `serde` for config, `dirs` for config and cache paths, `anyhow`, `thiserror`.
- Music.app is driven with JavaScript for Automation (JXA) through `osascript`.
  Every call returns JSON on stdout.

Measured on this machine: one `osascript` round trip is about 140 ms; a JXA dump
of the full 6,474-track library with five fields takes about 0.3 s. The tap
delivers 48 kHz stereo `f32` at about 86 callbacks per second.

## 4. Architecture

Single binary crate `appytui`. Five modules with one job each, wired together by
`main.rs`.

```
src/
  main.rs        terminal setup, event loop, wiring
  app/
    mod.rs       App state, Action enum, reduce()
    views.rs     per-tab list state, drill-down, filtering
    queue.rs     play context and queue derivation
  music/
    mod.rs       MusicBridge trait, Command/Event enums, worker thread
    jxa.rs       JXA scripts and osascript runner (real bridge)
    model.rs     Track, Playlist, PlayerStatus, PlayerState, RepeatMode
    fake.rs      in-memory fake bridge for tests
  viz/
    mod.rs       Visualizer trait, Frame type, spawn helper
    tap.rs       Core Audio tap + aggregate device (cidre), sample ring buffer
    spectrum.rs  FFT, log-spaced bands, smoothing, auto-gain (pure, tested)
    simulated.rs sine-based fallback
  art/
    mod.rs       ArtLookup: iTunes search + disk cache, worker thread
    render.rs    image -> half-block cell grid (pure, tested)
  ui/
    mod.rs       layout + draw(frame, &App)
    visualizer.rs Frame + VizSettings -> cells (pure, tested)
    tabs.rs, list.rs, now_playing.rs, status.rs, help.rs
  config/
    mod.rs       Config struct, load/create ~/.config/appytui/config.toml
    theme.rs     Theme struct, cava INI parser, built-in themes, lookup order
```

Threads:

- Main thread: crossterm events, a 1 Hz tick, and one `crossbeam-channel`
  select over the bridge, visualizer, and art result channels. Redraws only
  when something changed or the visualizer produced a frame.
- Bridge worker: consumes `music::Command`, runs `osascript`, sends
  `music::Event`. Commands are serialized so the UI never blocks on AppleScript.
- Audio callback (Core Audio real-time thread): copies samples into a lock-free
  ring buffer. No allocation, no locks.
- Spectrum thread: at 30 fps reads the latest window from the ring buffer,
  computes a `viz::Frame`, and sends it. Sleeps while the player is not playing.
- Art worker: fetches and decodes cover images on track change.

## 5. Music bridge

```rust
pub trait MusicBridge: Send {
    fn load_library(&mut self) -> Result<Vec<Track>>;
    fn load_playlists(&mut self) -> Result<Vec<Playlist>>;
    fn status(&mut self) -> Result<PlayerStatus>;
    fn play_track(&mut self, track: &TrackId, context: Option<&PlaylistId>) -> Result<()>;
    fn play_pause(&mut self) -> Result<()>;
    fn next(&mut self) -> Result<()>;
    fn previous(&mut self) -> Result<()>;
    fn seek(&mut self, seconds: f64) -> Result<()>;
    fn set_volume(&mut self, percent: u8) -> Result<()>;
    fn set_shuffle(&mut self, on: bool) -> Result<()>;
    fn set_repeat(&mut self, mode: RepeatMode) -> Result<()>;
}
```

Model:

- `Track { id: TrackId (persistent ID string), name, artist, album, album_artist,
  duration_secs: f64, track_number: u32, disc_number: u32, year: u32 }`
- `Playlist { id: PlaylistId (persistent ID), name, kind: Library | User | Smart,
  track_ids: Vec<TrackId> }`
- `PlayerStatus { state: Playing | Paused | Stopped, track_id: Option<TrackId>,
  position_secs: f64, volume: u8, shuffle: bool, repeat: Off | One | All }`

The JXA bridge launches Music.app if it is not running. Each method is one
script; the runner passes arguments as JSON via an environment variable, never
by string interpolation into the script. The bridge worker polls `status()`
once per second and only emits an event when the status changed.

`play_track` with a context plays `track X of playlist Y` so Music.app
continues through that playlist natively. Without a context it plays the
library track.

## 6. App state and reducer

`App` holds: loaded library and playlists, derived album and artist indexes,
the active tab, per-tab list cursors and drill-down state, the filter query,
the current `PlayerStatus`, the play context (see queue), the latest visualizer
frame, the current cover art grid, a transient status message, and flags such
as help-overlay open.

`reduce(&mut App, Action) -> Vec<Effect>` is pure with respect to I/O. Actions
are key presses, tick, bridge events, visualizer frames, and art results.
Effects are `SendCommand(music::Command)`, `LookupArt(Track)`, and `Quit`.
`main.rs` executes effects. This keeps all key handling and list logic unit
testable with the fake bridge.

Albums are grouped by `(album_artist or artist, album)` and artists by
`album_artist or artist`, both derived once after the library loads. Sorting is
case-insensitive with a leading "The " ignored.

Filtering uses `nucleo-matcher` over `"name artist album"` for the current
list. The filter is per tab and cleared with `Esc`.

## 7. Queue and play context

Music.app's AppleScript interface does not expose Up Next. The app keeps its
own play context: the ordered list of track IDs the user played from plus the
index of the current track. Playing a track from any list sets the context to
that list. On each status event the index is re-synced by locating the reported
track ID in the context (nearest occurrence after the old index, else first).
The Queue tab shows the context from the current index onward. When shuffle is
on the queue shows the context unordered with a "shuffle on" note, since the
real order is unknown.

## 8. Visualizer

`viz::Frame` carries per-channel bar heights in `0.0..=1.0`: `left: Vec<f32>`,
`right: Vec<f32>`, plus `waveform: Vec<f32>` (recent samples in `-1.0..=1.0`)
when waveform mode is on.

Tap (`viz/tap.rs`): a stereo global process tap excluding no processes, wrapped
in a private aggregate device on the default output device, exactly as in the
cidre `core-audio-record` example. The IO proc writes interleaved L/R samples
into a single-producer single-consumer ring buffer of 8192 frames. When the
default output device changes the tap is torn down and rebuilt.

Spectrum (`viz/spectrum.rs`), pure and tested, per channel:

1. Take the newest 2048 samples, apply a Hann window, run a real FFT.
2. Compute magnitudes for bins between `lower_cutoff_freq` and
   `higher_cutoff_freq` (defaults 50 Hz and 5 kHz, as in the user's cava
   config).
3. Map to N bars on a log-frequency scale, averaging the bins in each band.
   N is derived from the pane width, `bar_width`, and `bar_spacing`.
4. Convert to dB, clamp to a 60 dB range, normalize to 0..1.
5. Smoothing, matching cava: integral filter and gravity fall governed by
   `noise_reduction` (0..100, default 77), optional `monstercat` neighbour
   spreading, optional `waves` (wider, stronger spreading), and `autosens`
   auto-gain tracking the recent peak.

Mono mode averages the channels (or takes `left`/`right` per `mono_option`)
before analysis. Stereo mode analyses both and the renderer mirrors them with
the lowest bands meeting in the centre.

Rendering (`ui/visualizer.rs`), all pure functions over a `Frame` and a
`VizSettings`:

- `orientation = bottom | top | horizontal`. `horizontal` grows bars up and
  down from a centre line; with `horizontal_stereo = true` the left channel is
  drawn above the line and the right below.
- `channels = stereo | mono`, `reverse`.
- `bar_width` (default 2) and `bar_spacing` (default 1) in cells. Partial cells
  use `▁▂▃▄▅▆▇█` (and `▔`-style top eighths for `top` orientation).
- `gradient`: up to 8 colour stops blended vertically per cell row.
  `horizontal_gradient` gives one colour per bar across the width;
  `blend_direction` mixes the two as in cava. No gradient means the theme's
  foreground colour.
- `waveform = true` draws the oscilloscope-style waveform instead of bars.
- `show_idle_bar_heads` draws a flat line of bar tops while silent.

The `v` key toggles the visualizer, `V` cycles orientation, `w` toggles
waveform. Changes made with keys are not persisted; the config file is the
source of truth.

Fallback (`viz/simulated.rs`): the sine-plus-noise animation from
AppleMusicTUI, rendered through the same renderer so it honours the same
settings, used when tap creation fails. The status bar shows a one-line hint:
"Visualizer simulated. Allow system audio recording in System Settings >
Privacy & Security > Screen & System Audio Recording, then restart."

`--no-viz` skips the tap entirely.

## 8a. Configuration and themes

Config file: `~/.config/appytui/config.toml`, created with commented defaults
on first run. Sections:

```toml
[visualizer]
enabled = true
orientation = "horizontal"      # bottom | top | horizontal
channels = "stereo"             # stereo | mono
mono_option = "average"         # left | right | average
reverse = false
horizontal_stereo = false
bar_width = 2
bar_spacing = 1
lower_cutoff_freq = 50
higher_cutoff_freq = 5000
framerate = 30
autosens = true
sensitivity = 100
noise_reduction = 77
monstercat = true
waves = true
waveform = false
show_idle_bar_heads = true

[theme]
name = "catppuccin-mocha"       # built-in name, or a file under the theme dirs

[art]
enabled = true
```

Themes:

- A theme provides the visualizer gradient (1 to 8 stops), an optional
  horizontal gradient, `blend_direction`, background and foreground, and UI
  accent colours (selection, tab highlight, progress bar, dim text).
- Theme files use cava's INI format so existing cava themes load unchanged.
  Lookup order for `theme.name`: `~/.config/appytui/themes/<name>`,
  `~/.config/cava/themes/<name>`, then the built-in set. A cava theme has no UI
  accents, so those are derived from its gradient: accent = first stop,
  highlight = middle stop, dim = terminal default.
- Built-in themes: `catppuccin-mocha`, `catppuccin-mocha-h`, `solarized-dark`,
  `tricolor`, and `terminal` (no gradient, terminal palette only). Built-in
  themes define UI accents explicitly.
- `--theme <name>` overrides the config for one run.

The defaults above reproduce the user's current cava setup: horizontal
orientation, stereo channels, monstercat and waves smoothing, 5 kHz cutoff,
catppuccin-mocha gradient.

## 9. Album art

Music.app does not expose artwork for streamed tracks. On track change the art
worker queries Apple's public iTunes Search endpoint with the artist and title,
takes the first song result's 100 px cover URL, downloads it, and caches the
JPEG under `~/Library/Caches/appytui/art/<hash of artist|album>.jpg`. Cache hits
skip the network. The image is decoded and resampled to the panel size and
rendered as `▄` cells with two vertical pixels per cell (top pixel background,
bottom pixel foreground). Any failure leaves the panel showing metadata only.
`--no-art` disables the feature.

## 10. UI

Layout, minimum 80x24:

```
┌ 1 Songs  2 Albums  3 Artists  4 Playlists  5 Search  6 Queue ───────────┐
│ list / drill-down (left, 60%)      │ Now Playing (right, 40%)            │
│ title      artist      album  len  │  [album art]                        │
│ ...                                │  Title                              │
│                                    │  Artist — Album                     │
│                                    │  ▶ 1:23 ━━━━━━━━───── 3:57          │
│                                    │  ▁▂▃▅▇█▇▅▃▂▁▂▃▅▇█▇▅▃▂▁▂▃▅          │
│                                    │  shuffle off · repeat all · vol 80  │
├ status message ────────────────────┴── j/k move  Enter play  ? help ────┤
```

Albums and Artists tabs drill down: `Enter` on an album or artist opens its
track list in the same pane, `Backspace` returns.

Keys:

| Key | Action |
|---|---|
| `1`–`6` | switch tab |
| `j`/`k`, `↓`/`↑` | move cursor |
| `g`/`G` | top / bottom |
| `Ctrl-d`/`Ctrl-u` | half page |
| `Enter` | play selected (or drill down) |
| `Backspace` | back out of drill-down |
| `Space` | play / pause |
| `n`/`p` | next / previous |
| `←`/`→` | seek −5 s / +5 s |
| `+`/`-` | volume ±5 |
| `s` / `r` | toggle shuffle / cycle repeat |
| `/` | filter current list; `Esc` clears |
| `v` | toggle visualizer |
| `V` | cycle visualizer orientation |
| `w` | toggle waveform mode |
| `?` | help overlay |
| `q` | quit |

Colours come from the active theme (see 8a). The `terminal` theme uses only
the terminal palette so light and dark terminals both work.

## 11. Error handling

- Music.app absent or refusing automation: startup shows the error and a hint
  to allow Automation for the terminal, then exits with code 1.
- A failed command shows the error in the status bar for 5 s. The app keeps
  running.
- Tap creation failure switches to the simulated visualizer silently apart from
  the status hint. Tap errors mid-run tear down the tap and switch as well.
- Art failures are logged at debug level and ignored.
- Panics restore the terminal via a panic hook before printing.

## 12. Testing

- `app`: reducer tests with the fake bridge for navigation, drill-down,
  filtering, play context and queue resync, key handling.
- `music::jxa`: parsing tests against captured JSON output for each script.
- `viz::spectrum`: a synthesized 440 Hz sine lights the expected band; silence
  yields all zeros; smoothing falls monotonically.
- `art::render`: a 2x2 test image maps to the expected cells and colours.
- `ui::visualizer`: golden tests for each orientation, stereo mirroring, bar
  width/spacing, and gradient colour per row.
- `config::theme`: the four shipped cava theme files parse to the expected
  stops; lookup order prefers user dirs over built-ins.
- `#[ignore]` integration test for the real bridge and the real tap, run
  manually on a Mac with Music.app.

## 13. Non-goals confirmed with the user

No MusicKit, no cava runtime dependency, no Apple ASCII logo, no mouse.
