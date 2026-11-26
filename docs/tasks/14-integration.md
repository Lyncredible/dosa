# 14 · Windows integration and UI verification

Exercise the emulator as a user launches it, using repeatable fixtures for public tests and a separately supplied local fixture for the Windows compatibility smoke.

- Run workspace formatting, lints, and tests. Add process or headless integration tests for startup options, mount types, and guest exit status where those paths changed.
- Launch a built Windows binary from a clean configuration. Open menus, choose a host mount, adjust settings, run a guest, move the pointer across the viewport and window boundary, and verify text geometry with visible output.
- Test the supplied local ZIP through the same application path. Record whether each run reached the menu, rendered a guest screen, accepted input, and exited or reported an unsupported behavior.
- Keep local paths, software names or assets from the local fixture, and screenshots showing them out of tracked or public artifacts.
- Keep native Linux and macOS checks in [06](06-platforms.md).

The opt-in `desktop_smoke` target can generate its own ZIP and exercise VGA presentation, Enter and Escape transitions, WAV capture, and paused inspection and stepping in a native window. Set `DOSA_SMOKE_GENERATED=1` to run it without a supplied archive. A caller-supplied ZIP can use the same native window test with its own scripted Enter and Escape sequence.

On Windows, run `cargo test --locked -p dosa-desktop windows_caption_routes_native_messages_and_builds_actions -- --ignored` in a desktop session. The test creates a Win32 window, sends non-client caption messages to Dosa's subclass, reads the native caption, and checks the command IDs registered on Windows popup menus. Use the built Dosa window for visual inspection, physical input, and native file-picker interaction.
