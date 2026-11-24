# 22 · Sound Blaster digital audio

Emulate Sound Blaster 2.0 at port 220h, IRQ7, and DMA channel 1. The guest environment advertises `BLASTER=A220 I7 D1 T3`. An FM driver can use ports 228h and 229h to reach the same OPL2 device as AdLib's 388h and 389h ports.

- DSP reset identification, version, direct DAC output, and 8-bit mono DMA playback use the existing 48 kHz PC speaker and FM output path.
- A generated COM configures the DSP and DMA registers, plays samples from DOS memory, and acknowledges the completion interrupt through its own IRQ7 handler.
- Unit coverage verifies DMA auto-init reload, pause/resume, page boundary wrapping, exit after the current block, and the DMA and test-register identification commands.
- Remaining compatibility work includes Creative ADPCM, high-speed DSP commands, Sound Blaster Pro and 16 models, recording, and validation with native DOS audio workloads.

The [Creative Sound Blaster Series Hardware Programming Guide](https://pdos.csail.mit.edu/6.828/2018/readings/hardware/SoundBlaster.pdf) documents the DSP commands and port map.
