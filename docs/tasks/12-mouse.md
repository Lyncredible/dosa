# 12 · Seamless mouse

Map pointer position from the desktop content area into bounded DOS coordinates. Let the pointer enter and leave the window freely and keep native menu input separate. Expose a built-in two-button DOS mouse with a tested INT 33h interface.

- Reuse the renderer's guest viewport for pointer conversion, including resize, DPI, aspect correction, and letterboxing.
- Test reset, position and button polling, guest range changes, visibility state, and motion counters as supported.
- Release guest buttons on focus loss and ensure moving the pointer outside the active viewport can return control to the host.
- Validate native entry, exit, clicks, and menu interactions on Windows. Track driver callbacks, serial mouse, and PS/2 interfaces as separate compatibility work if applications require them.

DOSBox Staging provides a built-in DOS mouse driver and DOSBox-X describes both mouse capture and seamless integration. [DOSBox Staging mouse manual](https://www.dosbox-staging.org/0.83/manual/input/mouse/) [DOSBox-X mouse guide](https://dosbox-x.com/wiki/Guide%3AMouse-support-in-DOSBox%E2%80%90X)
