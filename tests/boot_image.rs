use std::process::Command;

#[test]
fn boots_raw_floppy_and_hard_disk_to_a_second_stage() {
    let scratch = tempfile::tempdir().unwrap();
    for (drive, second_stage_lba, sectors) in [('A', 1, 2880), ('C', 2, 4)] {
        let mut image = vec![0; 512 * sectors];
        let stage_one: &[u8] = if drive == 'A' {
            &[
                0xbb, 0x00, 0x7e, // MOV BX,7E00h
                0xb9, 2, 0, // MOV CX,2
                0xb8, 1, 2, // MOV AX,0201h
                0xcd, 0x13, // Read sector 2 into 0000:7E00, with the boot drive in DL
            ]
        } else {
            &[
                0xbe, 0x40, 0x7c, // MOV SI,7C40h
                0xb4, 0x42, 0xcd, 0x13, // Extended read from the packet
            ]
        };
        image[..stage_one.len()].copy_from_slice(stage_one);
        image[stage_one.len()..][..9].copy_from_slice(&[
            0x72, 5, // On carry, call INT 18h without jumping to stage two
            0xea, 0, 0x7e, 0, 0, // JMP FAR 0000:7E00h
            0xcd, 0x18,
        ]);
        image[0x40..0x50].copy_from_slice(&[0x10, 0, 1, 0, 0, 0x7e, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
        image[510..512].copy_from_slice(&[0x55, 0xaa]);
        let stage_two = [
            0xb8, b'O', 0x0e, 0xcd, 0x10, // BIOS teletype
            0xb0, b'K', 0xcd, 0x10, 0xcd, 0x18, // No next boot device
        ];
        image[512 * second_stage_lba..][..stage_two.len()].copy_from_slice(&stage_two);
        let path = scratch.path().join(format!("{drive} boot disk.img"));
        std::fs::write(&path, &image).unwrap();
        let command = format!("BOOT {drive} \"{}\"", path.display());
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", "--exec", &command])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        assert_eq!(output.stdout, b"OK", "{output:?}");
        assert_eq!(std::fs::read(path).unwrap(), image);
    }
}

#[test]
fn writable_hard_disk_boot_persists_only_the_requested_bios_sectors() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("writable hard disk.img");
    let mut image = vec![0; 512 * 4];
    let program = [
        0xc6, 0x06, 0, 0x06, b'C', // Set the data buffer's first byte.
        0xbb, 0, 0x06, 0xb9, 2, 0, 0xb8, 1, 3, 0xcd, 0x13, // CHS sector two.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, // Report status plus carry.
        0xc6, 0x06, 0, 0x06, b'E', 0xbe, 0, 0x7d, 0xb8, 0, 0x43, 0xcd, 0x13, // EDD LBA two.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, 0xc6, 0x06, 2, 0x7d, 1, 0xc6, 0x06, 8,
        0x7d, 4, // Reset count and seek past the end.
        0xb8, 0, 0x43, 0xcd, 0x13, 0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, 0xbb, 0, 0xff,
        0xb8, 1, 3, 0xcd, 0x13, // Write from across a DMA boundary.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, 0xbb, 0, 0x06, 0xb8, 2, 2, 0xcd,
        0x13, // Read both changed sectors.
        0xa0, 0, 0x06, 0xb4, 0x0e, 0xcd, 0x10, 0xa0, 0, 0x08, 0xcd, 0x10, 0xcd,
        0x18, // No next boot device.
    ];
    image[..program.len()].copy_from_slice(&program);
    image[0x100..0x110].copy_from_slice(&[0x10, 0, 1, 0, 0, 6, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    image[510..512].copy_from_slice(&[0x55, 0xaa]);
    image[512] = b'X';
    image[1024] = b'Y';
    std::fs::write(&path, &image).unwrap();

    let run = |option| {
        let command = format!("BOOT C {option}\"{}\"", path.display());
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args(["--headless", "--exec", &command])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        output.stdout
    };
    assert_eq!(run(""), b"4444XY");
    assert_eq!(std::fs::read(&path).unwrap(), image);
    assert_eq!(run("/rw "), b"005:CE");
    image[512] = b'C';
    image[1024] = b'E';
    assert_eq!(std::fs::read(&path).unwrap(), image);
    assert_eq!(run(""), b"4444CE");
    assert_eq!(std::fs::read(path).unwrap(), image);
}

#[test]
fn floppy_boot_can_write_to_a_blank_installation_disk() {
    use dosa_dos::{BootDrive, GuestSnapshotError, Machine};
    use std::fs::{self, File};

    let scratch = tempfile::tempdir().unwrap();
    let floppy_path = scratch.path().join("source floppy.img");
    let disk_path = scratch.path().join("blank hard disk.img");
    let mut floppy = vec![0; 1_474_560];
    let code = [
        0xa0, 0x75, 4, 0x04, b'0', 0xb4, 0x0e, 0xcd, 0x10, // Count hard disks in the BDA.
        0xb2, 0x80, 0xb4, 0x15, 0xcd, 0x13, // Detect a fixed hard disk.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, 0xc6, 0x06, 0, 6, b'H', 0xbb, 0, 6, 0xb9,
        2, 0, // Buffer and CHS sector two.
        0x31, 0xd2, 0xb8, 1, 3, 0xcd, 0x13, // Attempt to write to the source floppy.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, 0xb2, 0x80, 0xb8, 1, 3, 0xcd,
        0x13, // Write the same sector on the hard disk.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, 0x31, 0xd2, 0xb8, 1, 2, 0xcd,
        0x13, // Read the original floppy sector.
        0xa0, 0, 6, 0xb4, 0x0e, 0xcd, 0x10, 0xb2, 0x80, 0xb8, 1, 2, 0xcd,
        0x13, // Read the new hard disk sector.
        0xa0, 0, 6, 0xb4, 0x0e, 0xcd, 0x10, 0xcd, 0x18,
    ];
    floppy[..code.len()].copy_from_slice(&code);
    floppy[510..512].copy_from_slice(&[0x55, 0xaa]);
    floppy[512] = b'F';
    let mut disk = vec![0; 512 * 4];
    fs::write(&floppy_path, &floppy).unwrap();
    fs::write(&disk_path, &disk).unwrap();

    let open_with_disk = || {
        let mut machine = Machine::new();
        machine
            .boot_floppy_with_hard_disk(
                File::open(&floppy_path).unwrap(),
                File::options()
                    .read(true)
                    .write(true)
                    .open(&disk_path)
                    .unwrap(),
            )
            .unwrap();
        machine
    };
    let saved = open_with_disk().save_guest().unwrap();
    open_with_disk().load_guest(&saved).unwrap();
    let mut bare = Machine::new();
    bare.boot(File::open(&floppy_path).unwrap(), BootDrive::Floppy)
        .unwrap();
    assert!(matches!(
        bare.load_guest(&saved),
        Err(GuestSnapshotError::MediaChanged)
    ));

    let command = format!(
        "BOOT A /HDD \"{}\" \"{}\"",
        disk_path.display(),
        floppy_path.display()
    );
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--exec", &command])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(output.stdout, b"1340FH");
    assert_eq!(fs::read(&floppy_path).unwrap(), floppy);
    disk[512] = b'H';
    assert_eq!(fs::read(&disk_path).unwrap(), disk);
    assert!(matches!(
        open_with_disk().load_guest(&saved),
        Err(GuestSnapshotError::MediaChanged)
    ));
}

#[test]
fn boot_geometry_fits_the_image_and_extended_reads_reach_the_last_sector() {
    use dosa_dos::{BootDrive, Machine, RunState};
    use std::fs::File;
    use std::io::{Seek, SeekFrom, Write};

    let scratch = tempfile::tempdir().unwrap();
    for (sectors, expected_cx, expected_dx) in [(4_u64, 4, 1), (131_072, 0x813f, 0x0f01)] {
        let path = scratch.path().join(format!("geometry-{sectors}.img"));
        let code = [
            0xb4, 8, 0xcd, 0x13, // Query the maximum cylinder, head, and sector.
            0x31, 0xc0, 0xcd, 0x16, // Wait for the host to inspect the returned registers.
            0xb2, 0x80, 0xbe, 0x80, 0x7c, 0xb8, 0, 0x42, 0xcd, 0x13, // Read the last LBA.
            0x72, 7, 0xa0, 0, 6, 0xb4, 0x0e, 0xcd, 0x10, // Print the byte if the read worked.
            0xcd, 0x18,
        ];
        let mut boot = [0; 512];
        boot[..code.len()].copy_from_slice(&code);
        boot[0x80..0x88].copy_from_slice(&[0x10, 0, 1, 0, 0, 6, 0, 0]);
        boot[0x88..0x90].copy_from_slice(&(sectors - 1).to_le_bytes());
        boot[510..].copy_from_slice(&[0x55, 0xaa]);
        let mut file = File::create(&path).unwrap();
        file.set_len(sectors * 512).unwrap();
        file.write_all(&boot).unwrap();
        file.seek(SeekFrom::Start((sectors - 1) * 512)).unwrap();
        file.write_all(b"K").unwrap();
        drop(file);

        let mut machine = Machine::new();
        machine
            .boot(File::open(path).unwrap(), BootDrive::HardDisk)
            .unwrap();
        assert_eq!(machine.run(100).unwrap(), RunState::WaitingForKey);
        machine.set_paused(true);
        let registers = machine.paused_guest().unwrap().registers();
        assert_eq!(registers.cx, expected_cx);
        assert_eq!(registers.dx, expected_dx);
        machine.set_paused(false);
        machine.key(b' ', 0x39);
        assert_eq!(machine.run(100).unwrap(), RunState::Exited(1));
        assert_eq!(machine.take_output(), b"K");
    }
}

#[test]
fn headless_boot_observes_a_bios_timer_countdown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("slow boot.img");
    let mut image = vec![0; 1_474_560];
    let code = [
        0x31, 0xc9, 0x31, 0xd2, 0xb8, 0, 1, 0xcd, 0x1a, // Clear BIOS ticks.
        0x31, 0xc0, 0xcd, 0x1a, // Read BIOS ticks.
        0x83, 0xfa, 20, 0x72, 0xf7, // Poll for just over a second.
        0xb8, b'K', 0x0e, 0xcd, 0x10, 0xcd, 0x18, // No next boot device
    ];
    image[..code.len()].copy_from_slice(&code);
    image[510..512].copy_from_slice(&[0x55, 0xaa]);
    std::fs::write(&path, image).unwrap();
    let command = format!("BOOT A \"{}\"", path.display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--exec", &command])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert_eq!(output.stdout, b"K");
}

#[test]
fn restores_boot_disk_status_and_reads_the_second_stage() {
    use dosa_dos::{BootDrive, GuestSnapshotError, Machine, RunState};
    use std::fs::{self, File};

    let scratch = tempfile::tempdir().unwrap();
    for (drive, sectors) in [(BootDrive::Floppy, 2880), (BootDrive::HardDisk, 4)] {
        let original = scratch.path().join(format!("{drive:?}.img"));
        let altered = scratch.path().join(format!("{drive:?}-altered.img"));
        let mut image = vec![0; 512 * sectors];
        let code = [
            0x31, 0xc9, 0xb8, 1, 2, 0xcd, 0x13, // Read invalid sector zero.
            0x30, 0xe4, 0xcd, 0x16, // Wait for a key before querying status.
            0xb4, 1, 0xcd, 0x13, 0x80, 0xfc, 4, // Expect the saved sector-not-found error.
            0x75, 18, // Jump to INT 18h if the status differs.
            0xbb, 0, 0x7e, 0xb9, 2, 0, 0xb8, 1, 2, 0xcd, 0x13, // Read sector two.
            0x72, 5, 0xea, 0, 0x7e, 0, 0, // Jump to the loaded stage on success.
            0xcd, 0x18, // Stop booting.
        ];
        image[..code.len()].copy_from_slice(&code);
        image[510..512].copy_from_slice(&[0x55, 0xaa]);
        image[512..519].copy_from_slice(&[0xb8, b'K', 0x0e, 0xcd, 0x10, 0xcd, 0x18]);
        fs::write(&original, &image).unwrap();
        image[513] = b'Z';
        fs::write(&altered, &image).unwrap();

        let boot = |path: &std::path::Path| {
            let mut machine = Machine::new();
            machine.boot(File::open(path).unwrap(), drive).unwrap();
            machine
        };
        let mut running = boot(&original);
        assert_eq!(running.run(200).unwrap(), RunState::WaitingForKey);
        running.set_paused(true);
        let saved = running.save_guest().unwrap();

        let mut restored = boot(&original);
        restored.load_guest(&saved).unwrap();
        assert_eq!(
            restored.paused_guest().unwrap().state,
            RunState::WaitingForKey
        );
        restored.set_paused(false);
        restored.key(b' ', 0x39);
        assert_eq!(restored.run(200).unwrap(), RunState::Exited(1));
        assert_eq!(restored.take_output(), b"K");

        let mut changed = boot(&altered);
        assert!(matches!(
            changed.load_guest(&saved),
            Err(GuestSnapshotError::MediaChanged)
        ));
        assert_eq!(changed.run(200).unwrap(), RunState::WaitingForKey);
        assert!(matches!(
            Machine::new().load_guest(&saved),
            Err(GuestSnapshotError::MediaChanged)
        ));
        if drive == BootDrive::HardDisk {
            let mut writable = Machine::new();
            writable
                .boot_writable_hard_disk(
                    File::options()
                        .read(true)
                        .write(true)
                        .open(&original)
                        .unwrap(),
                )
                .unwrap();
            assert!(matches!(
                writable.load_guest(&saved),
                Err(GuestSnapshotError::MediaChanged)
            ));
            assert_eq!(writable.run(200).unwrap(), RunState::WaitingForKey);
            let saved_writable = writable.save_guest().unwrap();
            assert!(matches!(
                boot(&original).load_guest(&saved_writable),
                Err(GuestSnapshotError::MediaChanged)
            ));
            writable.load_guest(&saved_writable).unwrap();
        }
    }
}

#[test]
fn restored_boot_guest_detects_the_replacement_floppy_once() {
    use dosa_dos::{BootDrive, DosError, GuestSnapshotError, Machine, RunState};
    use std::fs::{self, File};

    let scratch = tempfile::tempdir().unwrap();
    let boot_path = scratch.path().join("boot.img");
    let data_path = scratch.path().join("data.img");
    let mut boot = vec![0; 1_474_560];
    let code = [
        0x31, 0xc0, 0xcd, 0x16, // Wait while the drive is changed.
        0x31, 0xd2, 0xb4, 0x15, 0xcd, 0x13, // Get the drive type.
        0x88, 0xe0, 0x04, b'0', 0xb4, 0x0e, 0xcd, 0x10, // Print type 2.
        0x30, 0xe4, 0xcd, 0x13, // Reset the controller.
        0xb4, 0x16, 0xcd, 0x13, // Detect the change and clear the latch.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, // Print status 6 plus carry.
        0xb4, 0x16, 0xcd, 0x13, // The next query should report no change.
        0x88, 0xe0, 0x14, b'0', 0xb4, 0x0e, 0xcd, 0x10, // Print status 0 plus carry.
        0xb4, 0x08, 0xcd, 0x13, // Get the new geometry.
        0x80, 0xe1, 0x3f, 0x88, 0xc8, 0x04, b'0', 0xb4, 0x0e, 0xcd, 0x10, 0xcd,
        0x18, // No next boot device.
    ];
    boot[..code.len()].copy_from_slice(&code);
    boot[510..512].copy_from_slice(&[0x55, 0xaa]);
    fs::write(&boot_path, boot).unwrap();
    fs::write(&data_path, vec![0; 368_640]).unwrap();
    let boot = || {
        let mut machine = Machine::new();
        machine
            .boot(File::open(&boot_path).unwrap(), BootDrive::Floppy)
            .unwrap();
        machine
    };
    let swap = |machine: &mut Machine| machine.swap_boot_floppy(File::open(&data_path).unwrap());
    assert!(matches!(
        swap(&mut Machine::new()),
        Err(DosError::NoBootFloppy)
    ));
    let mut running = boot();
    assert_eq!(running.run(100).unwrap(), RunState::WaitingForKey);
    running.set_paused(true);
    swap(&mut running).unwrap();
    let saved = running.save_guest().unwrap();

    let mut restored = boot();
    assert!(matches!(
        restored.load_guest(&saved),
        Err(GuestSnapshotError::MediaChanged)
    ));
    swap(&mut restored).unwrap();
    restored.load_guest(&saved).unwrap();
    restored.set_paused(false);
    restored.key(b' ', 0x39);
    assert_eq!(restored.run(200).unwrap(), RunState::Exited(1));
    assert_eq!(restored.take_output(), b"2709");
}

#[test]
fn rejects_a_raw_image_without_the_boot_signature() {
    let file = tempfile::NamedTempFile::new().unwrap();
    file.as_file().set_len(512).unwrap();
    let command = format!("BOOT C \"{}\"", file.path().display());
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--exec", &command])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing the 55 AA signature"));
}

#[test]
fn batch_cannot_boot_a_host_image() {
    let scratch = tempfile::tempdir().unwrap();
    let batch = scratch.path().join("START.BAT");
    std::fs::write(
        &batch,
        "@echo off\r\nif errorlevel 0 boot A \"absent host image.img\"\r\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount")
        .arg(format!("C={}", scratch.path().display()))
        .args(["--headless", "--exec", "START"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("boot host disk images from the interactive shell")
    );
}
