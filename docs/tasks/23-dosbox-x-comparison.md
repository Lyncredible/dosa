# 23 · DOSBox-X feature comparison

Dosa already has drive mounting, CPU and speed menus, TOML launch profiles, fullscreen, guest screenshots, and seamless mouse input. DOSBox-X's menus cover additional launch, input, storage, audio, and capture workflows. [DOSBox-X menus](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Drop%E2%80%90Down-Menus)

## Next desktop increments

| Feature | First increment and verification |
| --- | --- |
| Drive information and unmount | Implemented. The Drives menu shows source type and access mode and allows unmounting while idle. Regressions verify keyboard selection, batch locking, prompt fallback, and remounting. |
| Quick launch | Implemented. Pick a COM, EXE, or BAT from Drives. Its folder is temporarily mounted read-only on an unused drive. Tests cover sibling resources, nested batch calls, guest errors, and prompt restoration. Native picker interaction remains in [task 14](14-integration.md). |
| Sound controls | Master mute and volume implemented through the Audio menu. Callback tests verify gain changes on queued audio and that mute keeps the sample queue advancing. Per-device levels remain planned. |
| Text clipboard | Implemented. Edit copies the visible CP437 text screen and pastes through the BIOS keyboard queue with backpressure. Tests cover clipboard text conversion, editable shell paste, queue bounds, cancellation, and guest exit cleanup. |
| Display scaling | Implemented. View selects aspect-corrected fit or whole-pixel scaling with a fit fallback. The pointer uses the last presented viewport. Tests cover high-DPI geometry, mouse edges, and small surfaces. |

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
