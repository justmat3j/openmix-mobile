# Contributing

Thanks for helping improve OpenMix. Keep changes focused, explain user-visible
behavior in the pull request, and update documentation when capabilities or
privacy behavior change.

## Development

- Use Rust 1.95 or newer and install the Linux X11/ALSA development packages
  listed in [README.md](README.md).
- Run `cargo fmt --check` and `cargo check --locked` before opening a pull
  request. `cargo build --release` checks the optimized desktop build.
- Do not commit audio files, credentials, tokens, user paths, or generated build
  artifacts.
- Keep provider integrations on documented, authorized APIs. Do not add DRM
  bypasses, private endpoint scraping, or unauthorized stream downloads.

## Pull requests

Describe the problem, the change, and how it was checked. Include screenshots
only when the interface changed and make sure they contain no personal data.
