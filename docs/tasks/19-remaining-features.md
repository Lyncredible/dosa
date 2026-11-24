# 19 · Remaining feature completion

Complete the remaining roadmap and verification requirements with focused implementation and review PRs. Keep each feature open until its normal entry point and failure behavior are verified.

| Work | Required evidence |
| --- | --- |
| BIOS IRQ completion | Interrupt shadows, halted-guest wake, callable default timer and keyboard handlers, chained guest ISRs and INT 1Ch tests |
| Wider text | 132-column modes with memory stride, BDA, cursor, scrolling, menu selection and resized-window verification |
| Guest screenshot capture | Explicit host save action, correct indexed palette conversion, saved PNG dimensions and pixel checks |
| PIT and speaker | Channel 0 mode timing, channels 1/2, port 61h gating, synthetic tone samples and native audio playback |
| Writable storage | Explicit writable mount or overlay, create/write/truncate/rename/delete/directories, persistence and confinement tests |
| Disk media | Read-only FAT volume mounts accept `--mount-fat` or a profile flag. A generated FAT12 fixture verifies geometry, short and long names, DOS CLI access and unchanged image bytes. Partitioned images, ISO/MSCDEX and boot are tracked separately. |
| AdLib | FM device, register timing, synthesis and mixer validation |
| Sound Blaster | DSP, DMA, IRQ, digital samples and native mixer validation |
| Protected mode | Descriptor tables, transitions, memory semantics and exceptions for 286/386 profiles |
| DPMI | Host services exercised by a reproducible protected-mode workload |
| Snapshots | Versioned CPU/device state, mount identity, save/restore and invalid-state tests |
| Platforms | CI and native Windows/Linux/macOS desktop, input, resizing, fullscreen and mount verification |
| Complete game test | Play through the local test game, record completion evidence privately, turn emulator failures into public synthetic regressions |

VGA attribute palettes and graphics BIOS text are tracked in task 18. Batch files are implemented in PR #26, with further compatibility fixes driven by generated regressions.
