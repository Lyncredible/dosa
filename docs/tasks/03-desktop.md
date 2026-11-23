# 03 · GPU desktop

Build the native frontend in `crates/desktop`. Keep the guest display separate from native window state. Present guest frames through the GPU and route native keyboard events to the DOS input queue.

- Open a window on Windows and confirm that it draws, resizes, accepts input, and closes.
- Confirm text remains legible and guest geometry retains its aspect ratio as the window changes size.
- Keep the windowing and GPU interface portable for [06](06-platforms.md).
