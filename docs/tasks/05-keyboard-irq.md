# Keyboard IRQ1 for guest handlers

- [x] Deliver make and break scancodes to guest-installed real-mode INT 09h handlers.
- [x] Respect the CPU interrupt flag and the primary PIC IRQ1 mask. Hold additional IRQ1 requests until end of interrupt.
- [x] Expose the keyboard data and output-ready status ports and support ADh and AEh interface commands.
- [x] Preserve BIOS keyboard input and direct status and data port polling when no guest handler is installed.
- [x] Verify mask, interrupt flag, end of interrupt, and FIFO ordering with synthetic fixtures on Windows.

Follow-up compatibility work includes chaining to a callable BIOS keyboard handler, waking the CPU from HLT, and modeling the CPU interrupt shadow after STI and stack-segment changes.
