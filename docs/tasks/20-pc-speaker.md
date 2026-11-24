# 20 · PC speaker and native audio

Generate mono 48 kHz samples from PIT channel 2 and the port 61h speaker gate. Bound both emulation and native queues to 200 milliseconds. Native output adapts samples to the selected device's rate and channel count, and reports device initialization failures in the application.

- Verify channel 2 divisor loads, mode 2/3 waveforms, gate behavior, counter latching, and bounded sample queues.
- Run the opt-in native output test on a machine with an audio device.
- Verify the normal DOS port-I/O path and desktop audio drain.
- Complete channel 2 BCD and read-back support, and defer mode 2/3 reloads until the applicable cycle boundary.

Windows device tests and the normal DOS port-instruction path pass. The opt-in native audio test opened the default output stream and verified that its callback consumed the generated tone. Native Linux validation is in progress.

PIT channels 0 and 1 accept binary and packed BCD counts, count/status latching, and mode 0/4 single-shot and mode 2/3 periodic programming. Channel 1 updates the refresh toggle in port 61h. Tests follow the timing and read-back format in the [Intel 8254 data sheet](https://www.scs.stanford.edu/10wi-cs140/pintos/specs/8254.pdf).
