# 11 · CPU speed and instruction profiles

Expose a bounded emulation speed setting and CPU profiles backed by instruction support. Keep profile selection and execution pacing independent. A speed control changes how much CPU work happens between native frames. A profile controls instruction availability within the real-mode interpreter.

- Keep host pacing and labels explicit about what they measure. Preserve the instruction limit for unattended headless runs and keep the window responsive at every selectable speed.
- Give each CPU option documented constraints and test a generated program that behaves differently across profiles.
- Preserve the selected profile across a new DOS process and define what happens if the user changes it while a process is running.
- Test predictable timing behavior with the existing PIT channel 0 and IRQ0 path.

DOSBox-X exposes separate CPU type, core, and cycle controls. Its distinction informs the setting boundaries. [DOSBox-X CPU settings](https://dosbox-x.com/wiki/Guide%3ACPU-settings-in-DOSBox%E2%80%90X)
