# 09 · Guest timer interrupts

Support DOS programs that install an INT 08h handler and program the PC interval timer. Channel 0 counts at 1.193182 MHz with a default divisor of 65,536. Binary modes 2 and 3 support periodic interrupts and count reads. The shared primary PIC masks and prioritizes IRQ0 and IRQ1 and tracks end of interrupt.

- Synthetic tests cover programmed timing, repeated delivery to a guest ISR, PIC priority and EOI, and the default BIOS clock.
- The default BIOS clock uses host elapsed time. An installed INT 08h handler owns updates to the BIOS data-area tick count. Multiple timer edges between opportunities to service them coalesce into one pending PIC request.
- BIOS INT 1Ah supports setting and reading the BIOS tick counter. The 286 and 386 read and set packed BCD date and time through the DOS session clock. Guest saves preserve the clock and RTC daylight-saving option. [IBM BIOS Technical Reference](https://bitsavers.trailing-edge.com/pdf/ibm/pc/ps2/PS2_and_PC_BIOS_Interface_Technical_Reference_Apr87.pdf)
- Follow-up work includes callable default BIOS ISR chaining, dispatch to standalone INT 1Ch hooks, HLT wakeup, cycle-accurate mode 3 count reads, and PIT channels 1 and 2.
