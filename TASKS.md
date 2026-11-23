# dosa tasks

The first milestone is a GPU-rendered DOS prompt that can mount host content and run small real-mode COM and MZ programs on Windows. The next milestone expands executable and device coverage against reproducible DOS workloads. Linux and macOS share the Rust libraries and desktop frontend and have their own validation task.

| Task | Depends on | Status |
| --- | --- | --- |
| [01 · Real-mode CPU and memory](docs/tasks/01-cpu.md) | Architecture | Initial implementation complete |
| [02 · Host drive mounts](docs/tasks/02-mounts.md) | Architecture | Read-only mounts complete |
| [03 · GPU desktop](docs/tasks/03-desktop.md) | Architecture | Windows GPU smoke passed |
| [04 · DOS and first runnable milestone](docs/tasks/04-dos.md) | 01, 02, 03 | Initial implementation complete |
| [05 · DOS application compatibility](docs/tasks/05-compatibility.md) | 04 | In progress |
| [06 · Platform validation](docs/tasks/06-platforms.md) | 04 | Windows tested, Linux and macOS type-checked |
| [07 · CGA and planar VGA](docs/tasks/07-video.md) | 03, 04 | Initial modes complete |
| [08 · Guest keyboard interrupts](docs/tasks/08-keyboard.md) | 04 | Initial IRQ1 delivery complete |

See [architecture](docs/architecture.md) for the library boundaries and execution model.

Use one branch and worktree per task. Open a PR against the default branch and include its task, scope, and verification. A separate reviewer checks the diff and runs relevant tests before merging or sending findings back for revision. Update the linked task when work merges or its scope changes. Keep local fixtures and host paths out of commits, CI, and PR descriptions.
