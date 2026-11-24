# 14 · Windows integration and UI verification

Exercise the emulator as a user launches it, using repeatable fixtures for public tests and a separately supplied local fixture for the Windows compatibility smoke.

- Run workspace formatting, lints, and tests. Add process or headless integration tests for startup options, mount types, and guest exit status where those paths changed.
- Launch a built Windows binary from a clean configuration. Open menus, choose a host mount, adjust settings, run a guest, move the pointer across the viewport and window boundary, and verify text geometry with visible output.
- Test the supplied local ZIP through the same application path. Record whether each run reached the menu, rendered a guest screen, accepted input, and exited or reported an unsupported behavior.
- Keep local paths, software names or assets from the local fixture, and screenshots showing them out of tracked or public artifacts.
- Keep native Linux and macOS checks in [06](06-platforms.md).

The opt-in `desktop_smoke` target can generate its own ZIP and exercise VGA presentation, an Enter transition, and WAV capture in a native window. Set `DOSA_SMOKE_GENERATED=1` to run it without a supplied archive.
