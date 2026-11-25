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
