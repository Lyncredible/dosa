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
| CD-ROM and disk swapping | Read-only ISO 9660 primary volumes mount via CLI, launch profile, shell, and desktop. A generated DOS program verifies MSCDEX detection, version, ordered drive numbers, volume descriptors, and sector reads. Device-driver requests, per-drive image lists, and CUE audio remain planned. [ECMA-119](https://ecma-international.org/publications-and-standards/standards/ecma-119/) [DOSBox-X MSCDEX reference](https://dosbox-x.com/doxygen/html/dos__mscdex_8cpp_source.html) |
| Raw disk boot | `BOOT A` and `BOOT C` start a read-only raw floppy or hard disk image from a host path. Generated boot sectors verify the transfer to second-stage code through CHS or extended INT 13h reads. Full operating-system boot and disk swapping remain open. [DOSBox-X image management guide](https://dosbox-x.com/wiki/Guide%3AManaging-image-files-in-DOSBox%E2%80%90X) |
| Save states | Start with explicit save/load files for a real-mode guest. Verify CPU, device, and mount identity restoration before adding slots or automatic saves. See [task 19](19-remaining-features.md). |
| Joysticks and MIDI | Implement gameport input with a testable virtual joystick, then host controller mapping. Add MPU-401 UART separately with generated MIDI output and a configurable synthesis backend. |
| Audio capture and debugger | The Audio menu records the guest mix to a 48 kHz mono WAV. The Debug menu pauses and single-steps instructions, shows 16-bit registers, and inspects bytes at CS:IP, DS:SI, and SS:SP. Tests cover BIOS keyboard waits, frozen guest time, and VGA memory inspection without changing its latches. |
| Specialized hardware | Track printing, serial/modem, Ethernet, PC-98, and 3dfx against representative workloads after the core storage, protected-mode, and input gaps. |

DOSBox-X's image guide and feature overview describe the broader device and media coverage. [Image management guide](https://dosbox-x.com/wiki/Guide%3AManaging-image-files-in-DOSBox%E2%80%90X) [Feature highlights](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Feature-Highlights)
