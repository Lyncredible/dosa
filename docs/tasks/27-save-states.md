# 27 · Guest save states

Dosa owns guest execution and device state in the CPU and DOS libraries. The desktop should pause the guest at an instruction boundary and present explicit save and load actions. Before changing a running session, read and validate the complete file and prepare the devices and mount bindings it references.

| Work | Current state and verification |
| --- | --- |
| CPU payload | Version 1 stores processor profile, register and hidden descriptor state, A20, interrupt shadow, halt state, memory mapping bounds, and physical RAM. The header has a magic tag, a little-endian schema version, and a bounded metadata length. Metadata uses Postcard. RAM follows as raw bytes. Generated guests verify resume, protected paging state, and unchanged CPU state after rejecting malformed or incompatible data. |
| Video and device payloads | The core saves VGA planes, latches, registers, palette, and ANSI state and rebinds the aperture before restoring CPU memory. It preserves elapsed guest time, PIT and PIC, XMS, keyboard, mouse, joystick, PC speaker, and Sound Blaster DMA state. Tests compare frames after resuming and verify timer reloads and XMS handles. Live AdLib and configured MIDI currently prevent taking a snapshot. |
| DOS process and media | The core preserves the active process, memory allocator, pending calls, directories, open read-only files and shared seek positions. It checks mounted directory, single-file, ZIP, and FAT contents through the DOS filesystem and rejects changed media. Open files on writable mounts, shell pipes, optical media, and disk-boot sessions require additional work. |
| File and desktop controls | The core encodes a versioned payload with a length and CRC checksum. It validates and prepares replacement state before changing the machine. Generated guests verify input and exit after a restore on a fresh machine, plus rollback on damaged payloads and changed media. Add file handling and explicit desktop Save State and Load State actions, then verify both through the desktop. |

[Postcard wire format](https://postcard.jamesmunns.com/)
