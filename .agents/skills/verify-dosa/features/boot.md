# Boot a disk image

Use the DOS shell to boot a raw disk image and observe a guest loading its second stage through BIOS disk services.

## Sub-features

- `boot-media` starts a raw floppy or hard disk image.
- `boot-reject` reports invalid boot images.

## How to get to it (user POV)

- At the DOS prompt, use `BOOT A "floppy.img"` or `BOOT C "hard-disk.img"`.
- From the host CLI, pass the same DOS command after `--headless --exec`.

## Driving it with Cargo and PowerShell

- **Second stage.** Run `cargo test --locked --test boot_image boots_raw_floppy_and_hard_disk_to_a_second_stage`. The harness generates both images, drives `BOOT` through the real CLI, and asserts successful guest output from the second stage.
- **Rejected image.** Run `cargo test --locked --test boot_image rejects_a_raw_image_without_the_boot_signature`. The harness asserts the CLI fails and reports the missing signature.
- **Desktop prompt.** Copy a known-good raw image into `$scratch`. Open `& $dosa --mount "C=$media"`, type `BOOT A` followed by the image's absolute path in double quotes, and press Enter. Record the command and the fixture's expected boot output. If testing hard disk behavior, repeat in a fresh process with `BOOT C` and the matching hard disk image.

## Gotchas

- Use a fixture with a known boot result. A readable FAT filesystem alone does not establish that an image is bootable.
- The second-stage fixture establishes BIOS disk reads. Coverage of a full DOS installation requires the matching image.
