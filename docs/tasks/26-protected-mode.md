# 26 · Protected mode and DOS extenders

| Work | Verification |
| --- | --- |
| 286/386 mode entry and 386 return | Implemented LGDT, SGDT, LIDT, SIDT, LMSW and SMSW, plus 386 MOV CR0. Generated guests verify a 286 transition and a DOS COM program entering protection and returning to real mode before exit. |
| 16-bit ring 0 execution | GDT-backed code, data and stack segment caches enforce bounds and data access. The DOS COM regression dispatches a present 16-bit IDT gate and returns with IRET. The CPU reports unsupported loads and accesses at the failing instruction. |
| 32-bit execution and memory | Add full EIP and ESP, default operand and address sizes, 32-bit guests and gates, A20, physical memory above 1 MiB, and paging. Test transitions and wide addresses on the 386 profile. |
| Exceptions and privilege | Implement architectural faults, privilege checks and stack changes, LDTR and task state, and unsupported gate behavior. Verify guest fault handlers and restartable instructions. |
| DPMI | Add DOS extender host services and verify with a reproducible 32-bit DOS program. |

The first increment follows Intel's 80386 reference for table registers, CR0, segment descriptors, and gate formats. [Intel 80386 Programmer's Reference Manual](https://pdos.csail.mit.edu/6.828/2018/readings/i386.pdf)
