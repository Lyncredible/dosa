# 10 · Desktop controls

Provide accessible host controls for mounting a directory, file, or ZIP, selecting a drive, and adjusting emulator settings. Keep desktop menu events independent of DOS state and route their effects through the same application code as the shell and startup options.

- Use native file or folder selection where available. Send selected paths to the shared mount API and show errors in the application.
- Display the selected CPU profile, speed, and text mode. Report failed changes to the user and keep the displayed selection consistent with emulator state.
- Ensure host shortcuts and open menus consume their own keyboard and pointer events. Restore guest focus when the controls close.
- Exercise the menu with a real Windows window and verify settings after closing and reopening a menu.

The reference for scope is the Drive, CPU, and Video sections of the [DOSBox-X drop-down menu guide](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Drop%E2%80%90Down-Menus).
