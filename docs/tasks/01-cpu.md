# 01 · Real-mode CPU and memory

Implement the platform-independent execution core in `crates/cpu`. Run real-mode instructions with segmented addresses, 16-bit registers and flags, and a stack. Expose software interrupts to the DOS layer and allow the caller to limit instruction batches.

- Verify address wrapping, arithmetic flags, transfer of control, and stack effects with synthetic byte sequences.
- Verify faults identify unsupported instructions and the guest address.
- Exercise a COM-style program that issues a DOS interrupt.

Expand instruction coverage in [05](05-compatibility.md) based on reproducible failures.

The initial interpreter was merged in [PR #2](https://github.com/Lyncredible/dosa/pull/2) after nine Windows unit tests passed.
