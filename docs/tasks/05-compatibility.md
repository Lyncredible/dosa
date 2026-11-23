# 05 · DOS application compatibility

Drive compatibility with small documented workloads that can be reproduced from generated fixtures or redistributable binaries.

- Implement MZ loading with relocation, initial stack and entry registers, and a DOS environment.
- Expand DOS file, directory, process, and memory services as workloads exercise them.
- Expand CPU instruction and addressing coverage with instruction-level regressions.
- Add PC hardware behavior in increments, starting with timer and keyboard behavior, then VGA graphics modes, and then audio and additional devices as required by the workloads.
- Define write policy and DOS behavior for writable mounts before exposing mutation to guests.
- Record tested programs and their results, including precise gaps for workloads that reach an unsupported feature.

Keep each deliverable reviewable in a focused PR. Split larger increments into dedicated task files when they enter implementation.
