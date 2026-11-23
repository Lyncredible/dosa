# 15 · Reusable launch settings and fullscreen

Reduce repetitive setup by giving the application a reusable launch profile and a reversible fullscreen control, when each can be delivered in a focused change.

- Use typed settings for mounts, initial DOS command, CPU profile and speed, and text geometry. Keep an explicit CLI option authoritative when it overlaps a saved value and document how host-relative paths resolve.
- Keep reusable settings out of the guest filesystem. Verify invalid profiles produce actionable errors and a fixture profile can be run in headless and desktop modes.
- Provide a fullscreen toggle that preserves aspect correction, supports return to the prior window size, and keeps mouse entry and release predictable.
- Evaluate guest-frame screenshots separately after the live window path is stable.

The View menu provides Save screenshot with a native host save dialog. Capture encodes the current guest RGBA framebuffer as PNG. A round-trip regression verifies the dimensions and pixel bytes.

DOSBox Staging documents per-game configuration and capture. DOSBox-X provides fullscreen from both menus and startup settings. [DOSBox Staging configuration](https://www.dosbox-staging.org/0.83/manual/using-dosbox-staging/configuration/) [DOSBox Staging capture](https://www.dosbox-staging.org/0.83/manual/using-dosbox-staging/capture/) [DOSBox-X introduction](https://dosbox-x.com/wiki/Home.html)
