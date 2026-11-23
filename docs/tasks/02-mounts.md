# 02 · Host drive mounts

Implement host-backed DOS drives in `crates/mount`. Support directory mounts, single files exposed at the drive root, and direct ZIP mounts. Provide directory listing and file reads with case-insensitive DOS path lookup.

- Test DOS separators, case matching, nested paths, and predictable treatment of ambiguous names.
- Test a directory, standalone file, and both stored and deflated ZIP entries using synthetic fixtures.
- Verify path traversal and links outside a mounted directory cannot escape the drive root, and malformed ZIP entries report errors.

Initial mounts are read-only. Track file mutation semantics in [05](05-compatibility.md).

The mount crate has Windows tests for DOS name resolution, file and directory mounts, ZIP reads, and path confinement. The workspace headless test launches the same synthetic program from all three source types.
