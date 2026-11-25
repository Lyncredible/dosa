# 02 · Host drive mounts

Implement host-backed DOS drives in `crates/mount`. Support directory mounts, single files exposed at the drive root, and direct ZIP mounts. Provide directory listing and file reads with case-insensitive DOS path lookup.

- Test DOS separators, case matching, nested paths, and predictable treatment of ambiguous names.
- Test a directory, standalone file, and both stored and deflated ZIP entries using synthetic fixtures.
- Verify path traversal and links outside a mounted directory cannot escape the drive root, and malformed ZIP entries report errors.

Initial mounts are read-only. Track file mutation semantics in [05](05-compatibility.md).

The mount crate has Windows tests for DOS name resolution, file and directory mounts, ZIP reads, and path confinement. The workspace headless test launches the same synthetic program from all three source types.

## Writable ZIP copies

Use `--mount-session`, `session_zip = true` in a profile, or Drives → **Mount ZIP**. The mount materializes validated DOS paths into a temporary writable drive and retains the original archive. The existing ZIP entry and file limits apply, with an additional 512 MiB limit on initial expansion. **Mount read-only ZIP** keeps the original access mode available. DOS and profile integration tests verify write access, reuse within a batch, and unchanged source bytes.

## FAT disk images

Dosa mounts raw FAT volumes and the first primary FAT partition in MBR table order. The reader keeps all FAT seeks and reads within the declared volume. Tests exercise a generated FAT16 partition from DOS and reject overlapping and truncated layouts. [UEFI legacy MBR layout](https://uefi.org/specs/UEFI/2.11/05_GUID_Partition_Table_Format.html)
