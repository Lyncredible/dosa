# 08 · Guest keyboard interrupts

Support DOS programs that install an INT 09h handler and read keyboard make and break bytes through the emulated controller.

- Synthetic guests verify that delivery respects the interrupt flag, IRQ1 mask, and end-of-interrupt state. They also check ordered make and break bytes and the default BIOS keyboard path. [PR #12](https://github.com/Lyncredible/dosa/pull/12)
- Hardware-timed IRQ delivery, waking a halted guest, and chaining through the default BIOS IRQ1 handler remain compatibility work.
