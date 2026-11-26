use std::fs;
use std::process::Command;

use dosa::display::Desktop;
use dosa::shell::{Command as ShellCommand, Shell};
use dosa_desktop::{App, Input, Key, MenuAction, MountKind};
use dosa_mount::Mount;

const SECTOR: usize = 2048;

fn both_u16(bytes: &mut [u8], value: u16) {
    bytes[..2].copy_from_slice(&value.to_le_bytes());
    bytes[2..4].copy_from_slice(&value.to_be_bytes());
}

fn both_u32(bytes: &mut [u8], value: u32) {
    bytes[..4].copy_from_slice(&value.to_le_bytes());
    bytes[4..8].copy_from_slice(&value.to_be_bytes());
}

fn directory_record(block: u32, size: u32, directory: bool, identifier: &[u8]) -> Vec<u8> {
    let mut record = vec![0; (33 + identifier.len()).next_multiple_of(2)];
    record[0] = record.len() as u8;
    both_u32(&mut record[2..10], block);
    both_u32(&mut record[10..18], size);
    record[18..25].copy_from_slice(&[100, 1, 1, 0, 0, 0, 0]);
    record[25] = if directory { 2 } else { 0 };
    both_u16(&mut record[28..32], 1);
    record[32] = identifier.len() as u8;
    record[33..33 + identifier.len()].copy_from_slice(identifier);
    record
}

fn fixture() -> Vec<u8> {
    const SECTORS: u32 = 26;
    let mut image = vec![0; SECTORS as usize * SECTOR];
    let root = directory_record(20, 4096, true, &[0]);
    let metadata = &mut image[16 * SECTOR..17 * SECTOR];
    metadata[..7].copy_from_slice(b"\x01CD001\x01");
    metadata[8..72].fill(b' ');
    metadata[40..49].copy_from_slice(b"DOSA TEST");
    both_u32(&mut metadata[80..88], SECTORS);
    both_u16(&mut metadata[120..124], 1);
    both_u16(&mut metadata[124..128], 1);
    both_u16(&mut metadata[128..132], SECTOR as u16);
    both_u32(&mut metadata[132..140], 22);
    metadata[140..144].copy_from_slice(&18u32.to_le_bytes());
    metadata[148..152].copy_from_slice(&19u32.to_be_bytes());
    metadata[156..190].copy_from_slice(&root);
    metadata[881] = 1;
    image[17 * SECTOR..17 * SECTOR + 7].copy_from_slice(b"\xffCD001\x01");
    image[18 * SECTOR..18 * SECTOR + 22].copy_from_slice(
        b"\x01\x00\x14\x00\x00\x00\x01\x00\x00\x00\x03\x00\x16\x00\x00\x00\x01\x00BIN\x00",
    );
    image[19 * SECTOR..19 * SECTOR + 22].copy_from_slice(
        b"\x01\x00\x00\x00\x00\x14\x00\x01\x00\x00\x03\x00\x00\x00\x00\x16\x00\x01BIN\x00",
    );
    let root_records = [
        root,
        directory_record(20, 4096, true, &[1]),
        directory_record(22, 2048, true, b"BIN"),
        directory_record(23, 3, false, b"NOTES.TXT;1"),
    ]
    .concat();
    image[20 * SECTOR..20 * SECTOR + root_records.len()].copy_from_slice(&root_records);
    let mut newer = directory_record(24, 11, false, b"NOTES.TXT;2");
    newer[18..25].copy_from_slice(&[100, 2, 29, 15, 4, 6, 0]);
    image[21 * SECTOR..21 * SECTOR + newer.len()].copy_from_slice(&newer);
    let nested = [
        directory_record(22, 2048, true, &[0]),
        directory_record(20, 4096, true, &[1]),
        directory_record(25, 5, false, b"HELLO.COM;1"),
    ]
    .concat();
    image[22 * SECTOR..22 * SECTOR + nested.len()].copy_from_slice(&nested);
    image[23 * SECTOR..23 * SECTOR + 3].copy_from_slice(b"old");
    image[24 * SECTOR..24 * SECTOR + 11].copy_from_slice(b"ISO works\r\n");
    image[25 * SECTOR..25 * SECTOR + 5].copy_from_slice(&[0xb8, 7, 0x4c, 0xcd, 0x21]);
    image
}

#[test]
fn mounts_generated_iso_via_cli_profile_shell_and_desktop() {
    let scratch = tempfile::tempdir().unwrap();
    let image = scratch.path().join("data disc.iso");
    let before = fixture();
    fs::write(&image, &before).unwrap();
    let mount = Mount::open_iso_image(&image).unwrap();
    assert_eq!(
        mount
            .list("")
            .unwrap()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["BIN", "NOTES.TXT"]
    );
    assert!(!mount.is_writable() && mount.is_readonly("notes.txt").unwrap());
    assert_eq!(
        mount.volume_identity().unwrap().label.as_deref(),
        Some("DOSA TEST")
    );
    assert_eq!(mount.volume_identity().unwrap().serial, None);
    assert_eq!(
        mount
            .metadata("notes.txt")
            .unwrap()
            .modified
            .unwrap()
            .packed(),
        (0x285d, 0x7883)
    );
    assert!(mount.read("../NOTES.TXT").is_err());

    let profile = scratch.path().join("disc.toml");
    fs::write(&profile, "exec = 'D:\\BIN\\HELLO.COM'\n[[mounts]]\ndrive = 'D'\npath = 'data disc.iso'\niso_image = true\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--profile"])
        .arg(&profile)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(7), "{result:?}");

    let mut mount_arg = std::ffi::OsString::from("D=");
    mount_arg.push(&image);
    let result = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount-iso"])
        .arg(mount_arg)
        .args(["--exec", "TYPE D:\\NOTES.TXT"])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, b"ISO works\r\n");

    for command in ["MOUNT D /ISO", "IMGMOUNT D"] {
        let mut shell = Shell::new();
        shell
            .execute(&format!("{command} \"{}\"", image.display()))
            .unwrap();
        let ShellCommand::Print(text) = shell.execute("TYPE NOTES.TXT").unwrap() else {
            panic!("expected file text")
        };
        assert_eq!(text, b"ISO works\r\n");
    }

    let mut desktop = Desktop::new(Shell::new(), None);
    desktop.menu_action(MenuAction::Mount {
        drive: 'D',
        source: image.clone(),
        kind: MountKind::Iso,
    });
    assert_eq!(desktop.menu().drives[&'D'], "ISO image (read-only)");
    assert_eq!(fs::read(image).unwrap(), before);
}

#[test]
fn rejects_truncated_and_bad_directory_records() {
    let scratch = tempfile::NamedTempFile::new().unwrap();
    let mut image = fixture();
    fs::write(scratch.path(), &image[..image.len() - 1]).unwrap();
    assert!(Mount::open_iso_image(scratch.path()).is_err());
    image[20 * SECTOR + 6] ^= 1;
    fs::write(scratch.path(), &image).unwrap();
    assert!(Mount::open_iso_image(scratch.path()).is_err());
}

#[test]
fn save_state_rebinds_an_optical_image_and_checks_raw_sectors() {
    use dosa_dos::{GuestSnapshotError, Machine, RunState};

    let scratch = tempfile::tempdir().unwrap();
    let original = scratch.path().join("original.iso");
    let altered = scratch.path().join("altered.iso");
    let mut image = fixture();
    let wait_for_key = [0x30, 0xe4, 0xcd, 0x16];
    let exit = [0xb4, 0x4c, 0xcd, 0x21];
    image[25 * SECTOR..25 * SECTOR + 8].copy_from_slice(&[wait_for_key, exit].concat());
    // Update the recorded file length in BIN's third directory record.
    both_u32(&mut image[22 * SECTOR + 68 + 10..22 * SECTOR + 68 + 18], 8);
    fs::write(&original, &image).unwrap();
    // MSCDEX can read the system area even though the filesystem cannot list it.
    image[17] ^= 1;
    fs::write(&altered, &image).unwrap();
    let mount = |path: &std::path::Path| {
        let mut machine = Machine::new();
        machine
            .mount('D', Mount::open_iso_image(path).unwrap())
            .unwrap();
        machine
    };
    let mut guest = mount(&original);
    guest.launch('D', "BIN\\HELLO.COM", "").unwrap();
    assert_eq!(guest.run(100).unwrap(), RunState::WaitingForKey);
    let state = guest.save_guest().unwrap();

    let mut restored = mount(&original);
    restored.load_guest(&state).unwrap();
    restored.key(b'Q', 0x10);
    assert_eq!(restored.run(100).unwrap(), RunState::Exited(b'Q'));

    let mut changed_media = mount(&altered);
    assert!(matches!(
        changed_media.load_guest(&state),
        Err(GuestSnapshotError::MediaChanged)
    ));
}

#[test]
fn dos_program_discovers_iso_drives_and_reads_sectors_through_mscdex() {
    let scratch = tempfile::tempdir().unwrap();
    let image = scratch.path().join("compact disc.iso");
    let before = fixture();
    fs::write(&image, &before).unwrap();

    let mut absent = vec![0xbb, 0xff, 0xff, 0xb8, 0x00, 0x15, 0xcd, 0x2f];
    absent.extend([0x83, 0xfb, 0]); // CMP BX,0
    expect(&mut absent, 0x74, 1);
    absent.extend([0x3c, 0]); // CMP AL,0
    expect(&mut absent, 0x74, 2);
    absent.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    fs::write(scratch.path().join("ABSENT.COM"), absent).unwrap();

    let mut program = vec![0xb8, 0x00, 0x15, 0xcd, 0x2f];
    program.extend([0x83, 0xfb, 2]); // BX has two CD-ROM drives
    expect(&mut program, 0x74, 1);
    program.extend([0x83, 0xf9, 3]); // First is D (zero-based 3)
    expect(&mut program, 0x74, 2);
    program.extend([0xb8, 0x0c, 0x15, 0xcd, 0x2f, 0x81, 0xfb, 0x17, 0x02]);
    expect(&mut program, 0x74, 3);
    program.extend([0xb8, 0x0d, 0x15, 0xbb, 0, 0x20, 0xcd, 0x2f]);
    program.extend([0x81, 0x3e, 0, 0x20, 3, 5]); // D and F at ES:2000h
    expect(&mut program, 0x74, 4);
    program.extend([0xb8, 0x0b, 0x15, 0xb9, 2, 0, 0xcd, 0x2f, 0x3d, 0, 0]);
    expect(&mut program, 0x74, 5); // Folder drive C is not a CD-ROM
    program.extend([0xb8, 0x0b, 0x15, 0xb9, 5, 0, 0xcd, 0x2f, 0x3d, 0xd8, 0x5a]);
    expect(&mut program, 0x74, 6);
    program.extend([0x81, 0xfb, 0xad, 0xad]);
    expect(&mut program, 0x74, 7);

    program.extend([0xb8, 0x05, 0x15, 0xbb, 0, 0x20, 0x31, 0xd2, 0xcd, 0x2f]);
    expect(&mut program, 0x73, 8);
    program.extend([0x3d, 1, 0]); // Primary descriptor
    expect(&mut program, 0x74, 9);
    program.extend([0x81, 0x3e, 1, 0x20, b'C', b'D']);
    expect(&mut program, 0x74, 10);

    program.extend([
        0xb8, 0x08, 0x15, 0xba, 2, 0, 0x31, 0xf6, 0xbf, 24, 0, 0xcd, 0x2f,
    ]);
    expect(&mut program, 0x73, 11);
    program.extend([0x80, 0x3e, 0, 0x20, b'I']);
    expect(&mut program, 0x74, 12);
    program.extend([0x80, 0x3e, 0, 0x28, 0xb8]);
    expect(&mut program, 0x74, 13);

    program.extend([0xb8, 0x08, 0x15, 0xbe, 1, 0, 0xcd, 0x2f]); // Bad high word in sector address
    expect(&mut program, 0x72, 14);
    program.extend([0x3d, 21, 0]);
    expect(&mut program, 0x74, 15);
    program.extend([0x80, 0x3e, 0, 0x20, b'I']); // Rejected read leaves buffer intact
    expect(&mut program, 0x74, 16);
    program.extend([0xb8, 0x09, 0x15, 0xcd, 0x2f]);
    expect(&mut program, 0x72, 17);
    program.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    fs::write(scratch.path().join("CHECK.COM"), program).unwrap();

    let run = |command, with_images| {
        let mut process = Command::new(env!("CARGO_BIN_EXE_dosa"));
        process.args([
            "--headless",
            "--mount",
            &format!("C={}", scratch.path().display()),
        ]);
        if with_images {
            for drive in ['F', 'D'] {
                process.args(["--mount-iso", &format!("{drive}={}", image.display())]);
            }
        }
        let result = process.args(["--exec", command]).output().unwrap();
        assert!(result.status.success(), "{result:?}");
    };
    run("ABSENT", false);
    run("CHECK", true);
    assert_eq!(fs::read(image).unwrap(), before);
}

#[test]
fn dos_program_sends_mscdex_driver_requests_to_the_selected_iso() {
    let scratch = tempfile::tempdir().unwrap();
    let image = scratch.path().join("cd.iso");
    let before = fixture();
    fs::write(&image, &before).unwrap();

    fn set_word(code: &mut Vec<u8>, address: u16, value: u16) {
        code.extend([0xc7, 0x06]); // MOV word [address],value
        code.extend(address.to_le_bytes());
        code.extend(value.to_le_bytes());
    }
    fn expect_word(code: &mut Vec<u8>, address: u16, value: u16, error: u8) {
        code.extend([0x81, 0x3e]); // CMP word [address],value
        code.extend(address.to_le_bytes());
        code.extend(value.to_le_bytes());
        expect(code, 0x74, error);
    }
    fn request(code: &mut Vec<u8>, drive: u8, error: u8) {
        code.extend([0xb9, drive, 0, 0xb8, 0x10, 0x15, 0xcd, 0x2f]);
        expect(code, if drive == 5 { 0x73 } else { 0x72 }, error);
    }

    const PACKET: u16 = 0x1000;
    const CONTROL: u16 = 0x2000;
    const STATUS: u16 = PACKET + 3;
    let mut program = vec![
        0xbb, 0, 0x10, // MOV BX,PACKET
        0x8c, 0xd8, // MOV AX,DS
        0xa3, 0x10, 0x10, // MOV [PACKET+16],AX
    ];
    set_word(&mut program, PACKET, 26);
    set_word(&mut program, PACKET + 2, 3);
    set_word(&mut program, PACKET + 14, CONTROL);
    set_word(&mut program, PACKET + 18, 5);

    set_word(&mut program, CONTROL, 6); // Device capabilities
    set_word(&mut program, CONTROL + 2, 0xffff);
    set_word(&mut program, CONTROL + 4, 0xff);
    request(&mut program, 5, 1);
    expect_word(&mut program, STATUS, 0x0100, 2);
    expect_word(&mut program, PACKET, 0x011a, 3); // F is the second CD drive.
    expect_word(&mut program, CONTROL, 6, 4);
    expect_word(&mut program, CONTROL + 2, 0, 5);
    expect_word(&mut program, CONTROL + 4, 0, 6);

    set_word(&mut program, CONTROL, 7); // Size of a cooked sector
    request(&mut program, 5, 7);
    expect_word(&mut program, CONTROL + 2, SECTOR as u16, 8);
    expect_word(&mut program, PACKET + 18, 4, 9);
    set_word(&mut program, CONTROL, 0x0107); // Raw sector size is unsupported.
    request(&mut program, 5, 10);
    expect_word(&mut program, STATUS, 0x8103, 11);

    set_word(&mut program, CONTROL, 8); // Lead-out in absolute CD frames
    set_word(&mut program, PACKET + 18, 5);
    request(&mut program, 5, 12);
    expect_word(&mut program, CONTROL + 1, 26 + 150, 13);
    set_word(&mut program, CONTROL, 9); // Media check
    request(&mut program, 5, 14);
    expect_word(&mut program, CONTROL, 0x0109, 15);

    set_word(&mut program, PACKET, 27);
    set_word(&mut program, PACKET + 2, 0x80); // Read two cooked sectors
    set_word(&mut program, PACKET + 18, 2);
    set_word(&mut program, PACKET + 20, 24);
    request(&mut program, 5, 16);
    expect_word(&mut program, STATUS, 0x0100, 17);
    expect_word(&mut program, CONTROL, u16::from_le_bytes(*b"IS"), 18);
    expect_word(&mut program, CONTROL + SECTOR as u16, 0x07b8, 19);

    set_word(&mut program, PACKET + 20, 25); // End of volume
    request(&mut program, 5, 20);
    expect_word(&mut program, STATUS, 0x8108, 21);
    expect_word(&mut program, CONTROL, u16::from_le_bytes(*b"IS"), 22);
    set_word(&mut program, PACKET, 13); // Short read packet
    request(&mut program, 5, 23);
    expect_word(&mut program, STATUS, 0x8105, 24);
    request(&mut program, 2, 25); // Host directory is not a CD drive.
    program.extend([0x3d, 15, 0]); // AX=unknown drive
    expect(&mut program, 0x74, 26);
    expect_word(&mut program, STATUS, 0x8105, 27);
    program.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    fs::write(scratch.path().join("DRIVER.COM"), program).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args([
            "--headless",
            "--mount",
            &format!("C={}", scratch.path().display()),
            "--mount-iso",
            &format!("D={}", image.display()),
            "--mount-iso",
            &format!("F={}", image.display()),
            "--exec",
            "DRIVER.COM",
        ])
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(fs::read(image).unwrap(), before);
}

fn expect(program: &mut Vec<u8>, jump_if_passed: u8, error: u8) {
    program.extend([jump_if_passed, 5, 0xb8, error, 0x4c, 0xcd, 0x21]);
}

#[test]
fn desktop_swaps_iso_during_a_guest_and_reports_media_change_once() {
    let scratch = tempfile::tempdir().unwrap();
    let first = scratch.path().join("first.iso");
    let second = scratch.path().join("second.iso");
    let before = fixture();
    let mut after = before.clone();
    after[24 * SECTOR..24 * SECTOR + 3].copy_from_slice(b"NEW");
    fs::write(&first, &before).unwrap();
    fs::write(&second, &after).unwrap();
    fs::write(scratch.path().join("invalid.iso"), b"truncated").unwrap();

    fn expect_media(program: &mut Vec<u8>, value: u8, error: u8) {
        program.extend([
            0xb8, 0x10, 0x15, 0xbb, 0, 0x10, 0xb9, 3, 0, 0xcd, 0x2f, // Packet at ES:1000h
            0x80, 0x3e, 1, 0x20, value, // CMP byte [2001h],value
        ]);
        expect(program, 0x74, error);
    }
    fn expect_sector(program: &mut Vec<u8>, value: [u8; 2], error: u8) {
        program.extend([
            0xb8, 8, 0x15, 0xbb, 0, 0x30, 0xb9, 3, 0, 0xba, 1, 0, 0x31, 0xf6, 0xbf, 24, 0, 0xcd,
            0x2f, // Read sector 24 to ES:3000h
            0x81, 0x3e, 0, 0x30, value[0], value[1],
        ]);
        expect(program, 0x74, error);
    }
    let mut probe = vec![
        0xc7, 0x06, 0, 0x10, 26, 0, // Packet length
        0xc6, 0x06, 2, 0x10, 3, // IOCTL input
        0xc7, 0x06, 14, 0x10, 0, 0x20, // Control buffer at DS:2000h
        0x8c, 0x1e, 16, 0x10, // MOV [1010h],DS
        0xc7, 0x06, 18, 0x10, 2, 0, // Two control bytes
        0xc6, 0x06, 0, 0x20, 9, // Query media change
    ];
    expect_sector(&mut probe, *b"IS", 1);
    expect_media(&mut probe, 1, 2);
    probe.extend([0x31, 0xc0, 0xcd, 0x16]); // Wait for host swap and Enter.
    expect_media(&mut probe, 0xff, 3);
    expect_media(&mut probe, 1, 4);
    expect_sector(&mut probe, *b"NE", 5);
    probe.extend([0xb8, 0, 0x4c, 0xcd, 0x21]);
    fs::write(scratch.path().join("PROBE.COM"), probe).unwrap();
    fs::write(
        scratch.path().join("SWAP.BAT"),
        b"@ECHO OFF\r\nPROBE\r\nIF ERRORLEVEL 1 GOTO FAIL\r\nECHO SWAPPED>RESULT.TXT\r\nGOTO END\r\n:FAIL\r\nECHO FAILED AT CHECK %ERRORLEVEL%>RESULT.TXT\r\n:END\r\n",
    )
    .unwrap();
    let mut shell = Shell::new();
    shell
        .mount('C', Mount::open_writable_directory(scratch.path()).unwrap())
        .unwrap();
    shell
        .mount('D', Mount::open_iso_image(&first).unwrap())
        .unwrap();
    let mut desktop = Desktop::new(shell, Some("SWAP.BAT"));
    desktop.tick();
    desktop.tick();
    assert!(desktop.menu().running && desktop.menu().iso_drives.contains(&'D'));
    desktop.menu_action(MenuAction::Mount {
        drive: 'D',
        source: scratch.path().join("invalid.iso"),
        kind: MountKind::SwapIso,
    });
    assert!(desktop.menu().status.starts_with("ISO swap failed"));
    assert!(desktop.menu().running);
    desktop.menu_action(MenuAction::Mount {
        drive: 'D',
        source: second.clone(),
        kind: MountKind::SwapIso,
    });
    assert_eq!(desktop.menu().status, "Swapped ISO in D:");
    desktop.input(Input {
        key: Key::Enter,
        pressed: true,
        text: None,
        scan_code: 0x1c,
        modifiers: Default::default(),
    });
    for _ in 0..4 {
        desktop.tick();
    }
    assert!(!desktop.menu().running);
    assert_eq!(
        fs::read_to_string(scratch.path().join("RESULT.TXT")).unwrap(),
        "SWAPPED\r\n"
    );
    assert_eq!(fs::read(first).unwrap(), before);
    assert_eq!(fs::read(second).unwrap(), after);
}
