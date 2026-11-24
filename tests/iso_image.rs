use std::fs;
use std::process::Command;

use dosa::display::Desktop;
use dosa::shell::{Command as ShellCommand, Shell};
use dosa_desktop::{App, MenuAction, MountKind};
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
    let newer = directory_record(24, 11, false, b"NOTES.TXT;2");
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

    let mut shell = Shell::new();
    shell
        .execute(&format!("IMGMOUNT D \"{}\"", image.display()))
        .unwrap();
    let ShellCommand::Print(text) = shell.execute("TYPE NOTES.TXT").unwrap() else {
        panic!("expected file text")
    };
    assert_eq!(text, b"ISO works\r\n");

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
fn dos_program_discovers_iso_drives_and_reads_sectors_through_mscdex() {
    let scratch = tempfile::tempdir().unwrap();
    let image = scratch.path().join("compact disc.iso");
    let before = fixture();
    fs::write(&image, &before).unwrap();

    fn expect(program: &mut Vec<u8>, jump_if_passed: u8, error: u8) {
        program.extend([jump_if_passed, 5, 0xb8, error, 0x4c, 0xcd, 0x21]);
    }

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
