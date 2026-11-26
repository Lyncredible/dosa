# 26 · Protected mode and DOS extenders

| Work | Verification |
| --- | --- |
| 286/386 mode entry and 386 return | Implemented LGDT, SGDT, LIDT, SIDT, LMSW and SMSW, plus 386 MOV CR0. Generated guests verify a 286 transition and a DOS COM program entering protection and returning to real mode before exit. |
| 16-bit ring 0 execution | GDT-backed code, data and stack segment caches enforce bounds and data access. The DOS COM regression dispatches a present 16-bit IDT gate and returns with IRET. The CPU reports unsupported loads and accesses at the failing instruction. |
| 32-bit execution and memory | The 386 profile supports EIP and ESP above 64 KiB, descriptor-selected code and stack defaults, and 32-bit interrupt gates. A generated DOS program verifies instruction fetch, string copies, stack frames and IRETD above 64 KiB. The 286 and 386 profiles have 16 MiB of RAM with A20 controlled by BIOS, port 92h, and the keyboard controller output port. Tests cover address aliasing, instruction fetch, and protected descriptor tables above 1 MiB. Paging, extended-memory management, and reset through the gate ports remain open. [IBM System Control Port A reference](https://ardent-tool.com/docs/pdf/PS2_Model_70_Technical_Reference_Oct90.pdf) |
| Exceptions and privilege | Implement architectural faults, privilege checks and stack changes, LDTR and task state, and unsupported gate behavior. Verify guest fault handlers and restartable instructions. |
| DPMI | Add DOS extender host services and verify with a reproducible 32-bit DOS program. |

The processor behavior follows Intel's 80386 reference for table registers, CR0, segment descriptors, and gate formats. [Intel 80386 Programmer's Reference Manual](https://pdos.csail.mit.edu/6.828/2018/readings/i386.pdf)
