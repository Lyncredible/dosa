# 27 · Guest save states

Dosa owns guest execution and device state in the CPU and DOS libraries. The desktop should pause the guest at an instruction boundary and present explicit save and load actions. Before changing a running session, read and validate the complete file and prepare the devices and mount bindings it references.

| Work | Current state and verification |
| --- | --- |
| CPU payload | Version 1 stores processor profile, register and hidden descriptor state, A20, interrupt shadow, halt state, memory mapping bounds, and physical RAM. The header has a magic tag, a little-endian schema version, and a bounded metadata length. Metadata uses Postcard. RAM follows as raw bytes. Generated guests verify resume, protected paging state, and unchanged CPU state after rejecting malformed or incompatible data. |
| Video and device payloads | Capture the VGA planes, latch and register state, selected video mode and palette. Rebind the VGA aperture before restoring the CPU payload. Preserve guest time, PIT and PIC state, keyboard and mouse queues, and active audio state. Verify continued guest execution and matching frames after a restore. |
| DOS process and media | Preserve the active process, memory allocator, pending calls, open handles and positions, drive state, and mount identity. Verify that mismatched media cannot replace a running session and that loading a state preserves writable media consistency. |
| File and desktop controls | Write a versioned file containing the CPU, device, and process payloads with integrity checks. Add native Save State and Load State actions and verify a real-mode guest through the desktop entry points. Slots and automatic saves can follow. |

[Postcard wire format](https://postcard.jamesmunns.com/)
