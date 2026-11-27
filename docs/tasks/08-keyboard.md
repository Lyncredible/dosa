# 08 · Guest keyboard interrupts

Support DOS programs that install an INT 09h handler and read keyboard make and break bytes through the emulated controller.

- Synthetic guests verify that delivery respects the interrupt flag, IRQ1 mask, and end-of-interrupt state. They also check ordered make and break bytes and the default BIOS keyboard path. [PR #12](https://github.com/Lyncredible/dosa/pull/12)
- An enabled IRQ wakes a halted guest, and a guest ISR can chain through the default BIOS IRQ1 handler. Tests cover both paths and verify that the BIOS queues the resulting key once.
- The default INT 16h handler has a callable BIOS entry. Tests verify an empty-status poll and a blocking key read through a saved real-mode vector, plus a status poll reflected from a virtual 8086 monitor.
