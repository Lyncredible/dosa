# 06 · Platform validation

Validate the same source on Windows, Linux, and macOS. Keep headless execution and mount fixtures separate from physical GPU verification.

- Run workspace formatting, checks, and tests on each platform in CI.
- Launch the native desktop on each platform and exercise keyboard, resizing, GPU presentation, and clean shutdown.
- Smoke-test representative directory, file, and ZIP mounts on each platform. Include paths with spaces.
- Document platform prerequisites, verified graphics backends, and build instructions after testing them.

Windows Vulkan and Direct3D 12 passed the desktop GPU smoke test. Ubuntu 24.04 under WSLg passed the workspace suite and the Vulkan window smoke with an archive mount, one guest frame change after Enter, and clean shutdown. Physical Linux GPU, native picker, and macOS desktop runs remain open. WSLg audio validation requires an available ALSA PCM device.

The full workspace, including test targets, passed `cargo check --workspace --all-targets --locked` for `x86_64-unknown-linux-gnu` and `aarch64-apple-darwin` from a Windows host. The native macOS test suite and Metal presentation checks remain open.
