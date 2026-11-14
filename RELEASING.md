# Release checklist

1. Review source and history for personal data, local paths, credentials, and
   audio. Set a GitHub repository URL before publishing.
2. Confirm `cargo fmt --check`, `cargo check --locked`, and `cargo build
   --release --locked` pass on Linux X11.
3. Push the release commit and confirm the **CI** workflow passes.
4. Create a version tag such as `v1.0.0`. CI attaches a Linux x86_64 archive
   containing the executable, README, license, and third-party notices.
5. Smoke-test the archive on a clean Linux X11 install with an audio output
   device before attaching it to a GitHub Release.

The workflow builds Linux X11 only. Other operating systems and native
installers are not current release targets. CI does not publish a GitHub
Release automatically.
