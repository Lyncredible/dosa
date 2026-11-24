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
