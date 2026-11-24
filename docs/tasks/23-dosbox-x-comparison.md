# 23 · DOSBox-X feature comparison

Dosa already has drive mounting, CPU and speed menus, TOML launch profiles, fullscreen, guest screenshots, and seamless mouse input. DOSBox-X's menus cover additional launch, input, storage, audio, and capture workflows. [DOSBox-X menus](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Drop%E2%80%90Down-Menus)

## Next desktop increments

| Feature | First increment and verification |
| --- | --- |
| Drive information and unmount | Implemented. The Drives menu shows source type and access mode and allows unmounting while idle. Regressions verify keyboard selection, batch locking, prompt fallback, and remounting. |
| Quick launch | Implemented. Pick a COM, EXE, or BAT from Drives. Its folder is temporarily mounted read-only on an unused drive. Tests cover sibling resources, nested batch calls, guest errors, and prompt restoration. Native picker interaction remains in [task 14](14-integration.md). |
| Sound controls | Add master mute and volume. Verify output gain and continued guest audio timing while muted. Add per-device levels after that signal path is covered. |
| Text clipboard | Copy the visible DOS text screen with CP437 decoding. Add explicit paste through the DOS input queue, with tests for line endings, queue limits, and guest input timing. |
| Display scaling | Add integer scaling alongside the current aspect-corrected fit. Verify rendering and mouse coordinates at high DPI and on a window smaller than the guest frame. |

The DOSBox-X menu and clipboard guides document these workflows. [Menus](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Drop%E2%80%90Down-Menus) [Clipboard guide](https://dosbox-x.com/wiki/Guide%3AClipboard-support-in-DOSBox%E2%80%90X) [Scaling options](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Feature-Highlights)

## Compatibility and preservation

| Feature | First increment and verification |
| --- | --- |
| CD-ROM and disk swapping | Mount read-only ISO 9660 data, then implement MSCDEX queries and sector reads against a generated image. Add a per-drive image list and test swapping between two images. Treat CUE audio and disk boot as separate changes. |
| Save states | Start with explicit save/load files for a real-mode guest. Verify CPU, device, and mount identity restoration before adding slots or automatic saves. See [task 19](19-remaining-features.md). |
| Joysticks and MIDI | Implement gameport input with a testable virtual joystick, then host controller mapping. Add MPU-401 UART separately with generated MIDI output and a configurable synthesis backend. |
| Audio capture and debugger | Record emulator PCM to WAV and verify its sample rate and contents. Add paused register and memory inspection and single-step execution as a separate debugging increment. |
| Specialized hardware | Track printing, serial/modem, Ethernet, PC-98, and 3dfx against representative workloads after the core storage, protected-mode, and input gaps. |

DOSBox-X's image guide and feature overview describe the broader device and media coverage. [Image management guide](https://dosbox-x.com/wiki/Guide%3AManaging-image-files-in-DOSBox%E2%80%90X) [Feature highlights](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Feature-Highlights)
