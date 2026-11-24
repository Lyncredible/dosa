# 20 · PC speaker and native audio

Generate mono 48 kHz samples from PIT channel 2 and the port 61h speaker gate. Bound both emulation and native queues to 200 milliseconds. Native output adapts samples to the selected device's rate and channel count, and reports device initialization failures in the application.

- Verify channel 2 divisor loads, mode 2/3 waveforms, gate behavior, counter latching, and bounded sample queues.
- Run the opt-in native output test on a machine with an audio device.
- Verify the normal DOS port-I/O path and desktop audio drain.
- Complete PIT channel 1 and the remaining timing/BCD compatibility work in task 19.

Windows device tests and the normal DOS port-instruction path pass. The opt-in native audio test opened the default output stream and verified that its callback consumed the generated tone. Native Linux validation is in progress.
