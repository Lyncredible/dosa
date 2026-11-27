# 01 · Real-mode CPU and memory

Implement the platform-independent execution core in `crates/cpu`. Run real-mode instructions with segmented addresses, 16-bit registers and flags, and a stack. Expose software interrupts to the DOS layer and allow the caller to limit instruction batches.

- Verify address wrapping, arithmetic flags, transfer of control, and stack effects with synthetic byte sequences.
- Verify faults identify unsupported instructions and the guest address.
- Exercise a COM-style program that issues a DOS interrupt.

Expand instruction coverage in [05](05-compatibility.md) based on reproducible failures.

The initial interpreter was merged in [PR #2](https://github.com/Lyncredible/dosa/pull/2) after nine Windows unit tests passed.

The 386 profile implements all sixteen `SETcc` conditions for registers and memory. A regression checks the byte written, surrounding bytes, preserved flags, and the rejection on earlier CPU profiles. FreeDOS 1.4 uses `SETNE` when initializing an installed hard disk. [Intel 80386 reference](https://pdos.csail.mit.edu/6.828/2004/readings/i386/SETcc.htm)

The 386 profile implements 16-bit and 32-bit `SHLD` and `SHRD` with immediate or CL counts. The FreeDOS 1.4 installer uses `SHRD` after swapping to its first package disk. Regressions check the incoming source bits, memory operands, carry, sign changes, and masked counts. [SHLD reference](https://pdos.csail.mit.edu/6.828/2004/readings/i386/SHLD.htm) [SHRD reference](https://pdos.csail.mit.edu/6.828/2004/readings/i386/SHRD.htm)

The 386 profile implements `BT`, `BTS`, `BTR`, and `BTC`. Register operands mask the index to the operand width. Memory forms with a register index can address words or doublewords on either side of the base. Regressions cover both directions, immediate indices, and reporting the prior bit in carry. [Intel 80386 reference](https://pdos.csail.mit.edu/6.828/2004/readings/i386/BTS.htm)

All three CPU profiles implement `DAA` and `DAS` for packed decimal arithmetic. The regression covers decimal carry, borrow, a low-digit correction that wraps AL, and the sign, zero, and parity flags. [Intel instruction reference](https://www.intel.la/content/dam/www/public/us/en/documents/manuals/64-ia-32-architectures-software-developer-vol-2a-manual.pdf)
