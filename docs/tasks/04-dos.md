# 04 · DOS and first runnable milestone

Compose the CPU and mounts in `crates/dos`. Load COM and MZ executables and support DOS services for basic program output and exit. Expose keyboard, text, and VGA mode 13h display state to the application. Build the DOS-style prompt in the application for inspecting mounted drives and executing programs.

- Exercise a synthetic COM program from a mounted drive and verify its output and exit status.
- Exercise MZ relocation and verify that a synthetic guest can enter VGA mode 13h and draw pixels.
- Verify invalid executable paths and unsupported services return control cleanly.
- Exercise a host directory, file, and ZIP from the application's normal entry point.
- Run a local archive smoke test on Windows. Record the outcome and compatibility gaps without recording private fixture names or paths.

This milestone establishes the end-to-end execution path. Track broader executable support in [05](05-compatibility.md).

The DOS runtime tests cover COM output and exit, a relocated MZ program that draws into VGA memory, and a guest opening a mounted file relative to its current directory.
