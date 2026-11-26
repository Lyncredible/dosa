# dosa tasks

The first milestone is a GPU-rendered DOS prompt that can mount host content and run small real-mode COM and MZ programs on Windows. The next milestone expands executable and device coverage against reproducible DOS workloads. Linux and macOS share the Rust libraries and desktop frontend and have their own validation task.

| Task | Depends on | Status |
| --- | --- | --- |
| [01 · Real-mode CPU and memory](docs/tasks/01-cpu.md) | Architecture | Initial implementation complete |
| [02 · Host drive mounts](docs/tasks/02-mounts.md) | Architecture | Directory, ZIP, and writable session mounts implemented |
| [03 · GPU desktop](docs/tasks/03-desktop.md) | Architecture | Windows GPU smoke passed |
| [04 · DOS and first runnable milestone](docs/tasks/04-dos.md) | 01, 02, 03 | Initial implementation complete |
| [05 · DOS application compatibility](docs/tasks/05-compatibility.md) | 04 | In progress |
| [06 · Platform validation](docs/tasks/06-platforms.md) | 04 | Windows and WSLg smoke-tested, macOS type-checked |
| [07 · CGA and planar VGA](docs/tasks/07-video.md) | 03, 04 | Initial modes complete |
| [08 · Guest keyboard interrupts](docs/tasks/08-keyboard.md) | 04 | Initial IRQ1 delivery complete |
| [09 · Guest timer interrupts](docs/tasks/09-timer.md) | 04, 08 | Initial PIT channel 0 and IRQ0 delivery complete |
| [10 · Desktop controls](docs/tasks/10-desktop-controls.md) | 02, 03 | Initial controls complete |
| [11 · CPU speed and instruction profiles](docs/tasks/11-cpu-controls.md) | 01, 09 | Initial profiles and speed controls complete |
| [12 · Seamless mouse](docs/tasks/12-mouse.md) | 03, 04 | Initial implementation complete |
| [13 · More DOS text rows and columns](docs/tasks/13-text-modes.md) | 03, 04 | Initial implementation complete |
| [14 · Windows integration and UI verification](docs/tasks/14-integration.md) | 10, 11, 12, 13 | Windows integration smoke passed |
| [15 · Reusable launch settings and fullscreen](docs/tasks/15-launch-and-view.md) | 03, 10 | Initial profiles and fullscreen complete |
| [16 · Compatibility roadmap](docs/tasks/16-compatibility-roadmap.md) | 05, 09 | Planned |
| [17 · DOS batch files](docs/tasks/17-batch-files.md) | 04, 15 | Initial interpreter implemented |
| [18 · VGA sixteen-color graphics](docs/tasks/18-video16.md) | 07 | In progress |
| [19 · Remaining feature completion](docs/tasks/19-remaining-features.md) | 05, 06, 16 | In progress |
| [20 · PC speaker and native audio](docs/tasks/20-pc-speaker.md) | 09, 19 | In progress |
| [21 · AdLib FM audio](docs/tasks/21-adlib.md) | 19, 20 | Initial OPL2 emulation complete |
| [22 · Sound Blaster digital audio](docs/tasks/22-sound-blaster.md) | 19, 21 | Initial 8-bit playback complete |
| [23 · DOSBox-X feature comparison](docs/tasks/23-dosbox-x-comparison.md) | 10, 15, 16 | Prioritized |
| [24 · Desktop controls and appearance](docs/tasks/24-desktop-polish.md) | 03, 10, 23 | Reset, native Windows menu and caption, and icon implemented |
| [25 · MS-DOS 6.22 built-in command parity](docs/tasks/25-dos-command-parity.md) | 02, 04, 17 | Directory switches implemented. Command parity in progress |
| [26 · Protected mode and DOS extenders](docs/tasks/26-protected-mode.md) | 01, 05, 16, 19 | 16-bit and 32-bit ring 0 transitions and interrupt gates implemented |
| [27 · Guest save states](docs/tasks/27-save-states.md) | 01, 03, 04, 19 | Versioned CPU payload implemented |

See [architecture](docs/architecture.md) for the library boundaries and execution model.

Keep at most two workers active at a time. Each worker takes one implementation or review task before moving to the next.

Use one branch and worktree per task. Open a PR against the default branch and include its task, scope, and verification. A separate reviewer checks the diff and runs relevant tests before merging or sending findings back for revision. Use GitHub's **Rebase and merge** option or `gh pr merge --rebase` for every approved PR. Update the linked task when work merges or its scope changes. Keep local fixtures and host paths out of commits, CI, and PR descriptions.
