# 05 · DOS application compatibility

Drive compatibility with small documented workloads that can be reproduced from generated fixtures or redistributable binaries.

- Extend MZ compatibility against programs that exercise DOS environment and memory setup.
- Expand DOS file, directory, process, and memory services as workloads exercise them.
- Expand CPU instruction and addressing coverage with instruction-level regressions.
- Add PC hardware behavior in increments, starting with timer and keyboard behavior, then additional VGA behavior, audio, and other devices as required by the workloads.
- Add VGA mode 12h and its 640×480, 16-color scanout.
- Define write policy and DOS behavior for writable mounts before exposing mutation to guests.
- Record tested programs and their results, including precise gaps for workloads that reach an unsupported feature.

Keep each deliverable reviewable in a focused PR. Split larger increments into dedicated task files when they enter implementation.
