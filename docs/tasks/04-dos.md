# 04 · DOS and first runnable milestone

Compose the CPU and mounts in `crates/dos`. Build a DOS-style command prompt for inspecting mounted drives and executing programs. Load COM files with a program segment prefix and support DOS services for basic program output and exit. Expose keyboard and display state to the desktop.

- Exercise a synthetic COM program from a mounted drive and verify its output and exit status.
- Verify invalid executable paths and unsupported services return control cleanly.
- Exercise a host directory, file, and ZIP from the application's normal entry point.
- Run a local archive smoke test on Windows. Record the outcome and compatibility gaps without recording private fixture names or paths.

This milestone establishes the end-to-end execution path. Track broader executable support in [05](05-compatibility.md).
