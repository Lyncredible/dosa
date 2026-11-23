# 16 · Compatibility roadmap

Prioritize the next compatibility increment using public reproductions that identify the guest service or device blocking execution.

| Candidate | Bounded first increment |
| --- | --- |
| PC speaker | Emulate PIT channel 2 and port 61h gating, verify headless samples with a synthetic guest tone, then feed a bounded audio buffer in the native frontend. |
| Saved games and guest files | Design an explicit writable mount or copy-on-write overlay, specify DOS create, write, rename, and delete behavior, and test write confinement and persistence. |
| Disk media | Add one read-only disk image format backed by synthetic fixtures. Treat ISO/MSCDEX and booting as separate increments. |
| AdLib and Sound Blaster | Evaluate FM synthesis independently of Sound Blaster digital audio, which needs DMA, IRQ, timing, and a mixer. |
| Protected mode and snapshots | Expand CPU and memory semantics against a specific workload before DPMI. Define a versioned machine-state format and mount identity before save states. |

Original DOSBox documents writable host storage, image mounting, PC devices, and guest boot. Staging documents broader audio configuration, and DOSBox-X includes more image formats and save states. These are reference points for ordering work. [DOSBox manual](https://www.dosbox.com/DOSBoxManual.html) [DOSBox Staging PC speaker](https://www.dosbox-staging.org/0.83/manual/sound/sound-devices/pc-speaker/) [DOSBox Staging Sound Blaster](https://www.dosbox-staging.org/0.83/manual/sound/sound-devices/sound-blaster/) [DOSBox-X feature highlights](https://dosbox-x.com/wiki/DOSBox%E2%80%90X%E2%80%99s-Feature-Highlights)
