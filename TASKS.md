# dosa tasks

The first milestone is a GPU-rendered DOS prompt that can mount host content and run small real-mode COM and MZ programs on Windows. The next milestone expands executable and device coverage against reproducible DOS workloads. Linux and macOS share the Rust libraries and desktop frontend and have their own validation task.

| Task | Depends on | Status |
| --- | --- | --- |
| [01 · Real-mode CPU and memory](docs/tasks/01-cpu.md) | Architecture | In progress |
| [02 · Host drive mounts](docs/tasks/02-mounts.md) | Architecture | In progress |
| [03 · GPU desktop](docs/tasks/03-desktop.md) | Architecture | In progress |
| [04 · DOS and first runnable milestone](docs/tasks/04-dos.md) | 01, 02, 03 | In progress |
| [05 · DOS application compatibility](docs/tasks/05-compatibility.md) | 04 | Planned |
| [06 · Platform validation](docs/tasks/06-platforms.md) | 04 | Planned |

See [architecture](docs/architecture.md) for the library boundaries and execution model.

Use one branch and worktree per task. Open a PR against the default branch and include its task, scope, and verification. A separate reviewer checks the diff and runs relevant tests before merging or sending findings back for revision. Update the linked task when work merges or its scope changes. Keep local fixtures and host paths out of commits, CI, and PR descriptions.
