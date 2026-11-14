# OpenMix

OpenMix is a native Rust desktop music player built around **Mixing**. It plays
local audio, ranks compatible follow-up tracks, and creates an audible
crossfade. It uses eframe/egui for the native X11 window; there is no Electron,
browser view, bundled music, sample library, or bundled artwork.

## What works

- Import and play local MP3 and WAV files, seek, change volume, and use previous,
  next, shuffle, and repeat controls.
- Choose from four dark theme palettes in Settings; every theme keeps the red
  outline accent and is saved automatically.
- Search and sort the library, keep liked tracks, create and rename playlists,
  and manage the editable play queue.
- Analyze tempo, average energy, and a possible energy-rise cue across each full track on the device.
  Mixing ranks your imported tracks and builds a pitch-adjusted crossfade with
  a quieter early entrance and a peak limiter.
- Optionally ask OpenRouter's free Nemotron model to choose a phrase lead,
  incoming start point, early gain, and equal-power fade duration. OpenMix sends
  track metadata and local audio-analysis estimates, never audio or lyrics. Local mix
  planning remains available with the network feature disabled.
  and uses a labeled local transition plan if the model still returns no plan.
- Export a completed two-track or playlist mix as an MP4 through the native save
  dialog. MP4 export uses `ffmpeg`, which must be installed separately.

Tempo, energy-rise, and transition points are estimates. Listen before relying
on a suggested hand-off. Tempo correction uses resampling and may alter pitch.
The app starts with an empty library and only reads files you choose.

## Build on Linux

Rust 1.95 or newer and the system audio/X11 development packages are required.
On Debian or Ubuntu:

```sh
sudo apt-get install build-essential pkg-config libasound2-dev libx11-dev \
  libxrandr-dev libxinerama-dev libxcursor-dev libxi-dev libfontconfig1-dev \
  libgl1-mesa-dev
cargo run --release
```

Install `ffmpeg` if you want to export generated mixes as MP4:

```sh
sudo apt-get install ffmpeg
```

Build a distributable executable with:

```sh
cargo build --release
```

The executable is `target/release/openmix`. `make run`, `make build`, and
`make clean` are convenience targets. The current release workflow targets
Linux X11.

## Source layout

- `src/main.rs` owns application state, page flow, playback, and worker coordination.
- `src/ui.rs` contains the shared animated buttons, icons, theme cards, navigation, and volume controls.
- `src/theme.rs` owns palettes and egui visual configuration.
- `src/model.rs` contains persisted library models and worker messages.
- `src/storage.rs` handles the library file and secret storage.
- `src/metadata.rs` handles MusicBrainz metadata and cover-art downloads.
- `src/planning.rs` contains local and OpenRouter transition planning.
- `src/export.rs` handles mix source files and MP4 export.
- `src/audio.rs` contains decoding, analysis, transition rendering, and limiting.

## OpenRouter

Open **Settings**, paste an OpenRouter API key, save it, and choose **Mix now**
for an AI-planned transition. Clipboard support is enabled in the desktop app.
The key is stored with `secret-tool` when the desktop credential service is
available. If it is unavailable, OpenMix saves the key in its configuration
directory with owner-only permissions. `OPENROUTER_API_KEY` is also supported. The configured model is
`nvidia/nemotron-3-ultra-550b-a55b:free`.

Online features are opt-in in **Settings**. MusicBrainz lookups use the project
URL in the required User-Agent and need no API key. OpenRouter receives track
metadata and local audio-analysis estimates only; no audio or lyrics are sent.

## Data and privacy

OpenMix stores library metadata and preferences in the per-user configuration
directory and leaves music files at their original locations. It has no
telemetry or developer-operated data collection server. Online lookups can be
disabled in Settings. MusicBrainz receives track title and artist during lookup;
OpenRouter receives track metadata and audio-analysis estimates for AI mix
planning. [SECURITY.md](SECURITY.md) explains security reporting.

## Contributing

```sh
cargo fmt --check
cargo check --locked
```

See [CONTRIBUTING.md](CONTRIBUTING.md), [LICENSE](LICENSE), and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
