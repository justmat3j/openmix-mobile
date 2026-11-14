# OpenSync (OpenMix) Context

## Project Overview
OpenMix is a native Rust desktop music player focused on "Mixing". It allows users to play local audio (MP3, WAV), analyze track tempo and energy, and create AI-assisted or local crossfades between tracks.

## Key Features
- **Audio Playback**: Local MP3/WAV support, seek, volume, shuffle, repeat.
- **Library Management**: Search, sort, liked tracks, playlists, and play queue.
- **Mixing Engine**: Analyzes tempo and energy to rank follow-up tracks and build pitch-adjusted crossfades with peak limiting.
- **AI Integration**: Optional integration with NVIDIA NIM's Nemotron model for planning transitions (start points, gain, fade duration).
- **Privacy**: No telemetry; music files remain in place; AI receives only metadata and analysis estimates.

## Technical Stack
- **Language**: Rust (1.95+)
- **GUI**: eframe/egui (native X11)
- **OS Target**: Linux X11
- **Build Tools**: Cargo, Makefile

## Build Instructions (Linux)
Dependencies: `build-essential`, `pkg-config`, `libasound2-dev`, `libx11-dev`, `libxrandr-dev`, `libxinerama-dev`, `libxcursor-dev`, `libxi-dev`, `libfontconfig1-dev`, `libgl1-mesa-dev`.
Commands: `cargo run --release` or `make run`.

## Configuration
- API Key: NVIDIA NIM API key stored via `secret-tool` or local config.
- Model: `nvidia/nemotron-3.5-super-vl-120b-a12b` (endpoint: `https://integrate.api.nvidia.com`).
