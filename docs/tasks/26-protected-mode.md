# 26 · Protected mode and DOS extenders

| Work | Verification |
| --- | --- |
| 286/386 mode entry and 386 return | Implemented LGDT, SGDT, LIDT, SIDT, LMSW and SMSW, plus 386 MOV CR0. Generated guests verify a 286 transition and a DOS COM program entering protection and returning to real mode before exit. |
| 16-bit ring 0 execution | GDT-backed code, data and stack segment caches enforce bounds and data access. The DOS COM regression dispatches a present 16-bit IDT gate and returns with IRET. The CPU delivers segment faults with the faulting instruction address. |
| 32-bit execution and memory | The 386 profile supports EIP and ESP above 64 KiB, descriptor-selected code and stack defaults, and 32-bit interrupt gates. A generated DOS program verifies instruction fetch, string copies, stack frames and IRETD above 64 KiB. The 286 and 386 profiles have 16 MiB of RAM with A20 controlled by BIOS, port 92h, and the keyboard controller output port. Tests cover address aliasing, instruction fetch, and protected descriptor tables above 1 MiB. Reset through the gate ports remains open. [IBM System Control Port A reference](https://ardent-tool.com/docs/pdf/PS2_Model_70_Technical_Reference_Oct90.pdf) |
| XMS | The built-in DOS runtime exposes the XMS 2.0 far-call ABI on the 286 and 386 profiles. A generated COM guest verifies discovery, allocation, transfers, locking, resizing, and release. The driver supports HMA ownership and A20 control. Upper-memory blocks and XMS 3.x remain open. [XMS 2.0 specification](https://github.com/OpenRakis/Spice86/wiki/XMS-2.0-Specs) |
| Paging | The 386 profile walks 4 KiB page tables in ring 0, updates accessed and dirty bits, and delivers missing-page faults through the IDT. A generated guest maps a missing page and restarts a cross-page store. Translation caching, user page permissions, and fault escalation remain open. |
| Exceptions and privilege | Ring 0 guests receive segment-not-present, stack, and general-protection faults from descriptor checks and segment accesses. A generated 286 guest repairs descriptors, retries accesses, and handles an expand-down stack limit. Cross-privilege transfers, LDTR and task state, remaining fault classes, and fault escalation remain open. |
| DPMI | Add DOS extender host services and verify with a reproducible 32-bit DOS program. |

The processor behavior follows Intel's 80386 reference for table registers, CR0, segment descriptors, and gate formats. [Intel 80386 Programmer's Reference Manual](https://pdos.csail.mit.edu/6.828/2018/readings/i386.pdf)
