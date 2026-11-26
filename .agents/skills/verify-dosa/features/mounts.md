# Mounted drives

Expose host directories, files, ZIPs, FAT volumes, and ISO images as DOS drives. Verify both the DOS view and any changes to the backing media.

## Sub-features

- `mount-directory` reads a directory and persists changes when mounted writable.
- `mount-zip` reads a ZIP or makes a disposable writable session copy.
- `mount-images` reads FAT and ISO media.
- `mount-switch` selects or unmounts drives from the desktop.

## How to get to it (user POV)

- Use `--mount`, `--mount-rw`, `--mount-session`, `--mount-fat`, or `--mount-iso` at startup. Profiles can also declare mounts.
- At the desktop DOS prompt, use `MOUNT D "host path"` or put `/RW`, `/SESSION`, `/FAT`, or `/ISO` before the path. Enter `D:` and `DIR`.
- Open **Drives**, select the drive letter, and choose the corresponding mount action or unmount the selected drive.

## Driving it with PowerShell and Cargo

- **Directory.** Run the [shell recipe](shell.md) on a writable directory and reopen its output. For read-only behavior, start from a clean fixture with `RUN.BAT` only, use `--mount` in place of `--mount-rw`, and verify `RESULT.TXT` is absent afterward. Capture the guest diagnostic and host directory listing.
- **Writable guest operations.** Run `cargo test --locked --test headless guest_writable_mount_persists_file_and_directory_operations` for guest-visible operations and persisted host results.
- **ZIP session.** Run `cargo test --locked --test headless writable_zip_copy_allows_dos_read_write_access_without_changing_archive` to check guest writes and source archive integrity for CLI and profile session mounts. For a desktop ZIP write regression, mount a ZIP containing `RUN.BAT` via **Drives → Mount ZIP**, run `RUN PROBE`, and use `TYPE RESULT.TXT` before closing. Hash the source ZIP before and after, reopen it in a new session, and verify `RESULT.TXT` is absent.
- **FAT and ISO.** Run `cargo test --locked --test fat_image` and `cargo test --locked --test iso_image` for generated images, DOS directory and file reads, and documented formats. Exercise **Drives → Mount FAT image** or **Drives → Mount ISO image** with an authorized fixture copy when the native picker is in scope. Switch to the selected drive and compare `DIR` and `TYPE` against known fixture contents.
- **Drive selection and removal.** Mount separate fixture directories as `C:` and `D:` with different DOS 8.3 filenames. Switch to `D:` at the prompt and verify its `DIR`. Select `D:` in **Drives**, unmount it, and verify the prompt returns to `C:` and reports that `D:` is unavailable when selected again.

## Gotchas

- Create the host directory before using `--mount-rw`. Use DOS 8.3 names for files the guest creates.
- FAT and ISO mounts are read-only. ZIP session writes must disappear when the mount is replaced or Dosa exits.
- Change drives and mounts from the desktop menu while the shell is idle. A batch cannot mount a host path.
