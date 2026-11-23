# 08 · Guest keyboard interrupts

Support DOS programs that install an INT 09h handler and read keyboard make and break bytes through the emulated controller.

- Verify custom handler delivery honors the interrupt flag, IRQ1 mask, and end-of-interrupt state.
- Verify make and break bytes advance the guest's handler while the default BIOS keyboard path remains usable.
