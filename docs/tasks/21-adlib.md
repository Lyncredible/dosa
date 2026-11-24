# 21 · AdLib FM audio

DOS programs can write OPL2 registers at ports 388h and 389h and poll both timer flags. The DOS machine mixes the Rust OPL2 synthesizer with the PC speaker and forwards bounded mono 48 kHz PCM through the existing native audio stream. Each program launch resets the audio devices.

- Deterministic timer tests cover both prescalers, overflow, mask, reset, and periodic reload.
- A generated COM program programs the OPL2 voice through guest port instructions, reads the AdLib status, and plays both FM and PC speaker audio. The PCM test checks the mixed waveform, queue bound, and launch reset.
- FM synthesis uses [oplon](https://docs.rs/oplon/1.0.0/oplon/). Digital Sound Blaster playback remains in [task 19](19-remaining-features.md).
