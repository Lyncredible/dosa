use std::io::{Cursor, Seek, Write};
use std::process::Command;

use dosa::{display::Desktop, shell::Shell};
use dosa_desktop::{App, MenuAction, MountKind};
use dosa_mount::Mount;

#[test]
fn reads_a_generated_fat12_volume_through_dos() {
    let scratch = tempfile::tempdir().unwrap();
    let image = scratch.path().join("test floppy.img");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&image)
        .unwrap();
    file.set_len(1_474_560).unwrap();
    fatfs::format_volume(
        &mut file,
        fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat12),
    )
    .unwrap();
    {
        file.rewind().unwrap();
        let volume = fatfs::FileSystem::new(&mut file, fatfs::FsOptions::new()).unwrap();
        let root = volume.root_dir();
        let notes = root.create_dir("NOTES").unwrap();
        notes
            .create_file("Long Notes.txt")
            .unwrap()
            .write_all(b"FAT12 works\r\n")
            .unwrap();
    }
    drop(file);

    let before = std::fs::read(&image).unwrap();
    let mount = dosa_mount::Mount::open_fat_image(&image).unwrap();
    let listing = mount.list("notes").unwrap();
    assert_eq!(listing.len(), 1);
    assert_eq!(listing[0].host_name, "Long Notes.txt");
    assert_eq!(
        mount.read(&format!("NOTES/{}", listing[0].name)).unwrap(),
        b"FAT12 works\r\n"
    );
    assert!(mount.read("../NOTES").is_err());
    assert!(!mount.is_writable());

    let mut drive = std::ffi::OsString::from("A=");
    drive.push(&image);
    let result = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .args(["--headless", "--mount-fat"])
        .arg(drive)
        .args(["--exec", &format!("TYPE A:\\NOTES\\{}", listing[0].name)])
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    assert_eq!(result.stdout, b"FAT12 works\r\n");
    assert_eq!(std::fs::read(image).unwrap(), before);
}

#[test]
fn fat_attributes_match_dir_filters_and_guest_find() {
    use dosa_mount::attributes;

    let scratch = tempfile::tempdir().unwrap();
    let mut image = vec![0; 1_474_560];
    {
        let mut handle = Cursor::new(&mut image);
        fatfs::format_volume(
            &mut handle,
            fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat12),
        )
        .unwrap();
        handle.rewind().unwrap();
        let volume = fatfs::FileSystem::new(handle, fatfs::FsOptions::new()).unwrap();
        let root = volume.root_dir();
        for name in ["NORMAL.TXT", "MASKED.DAT"] {
            root.create_file(name).unwrap().write_all(b"x").unwrap();
        }
        root.create_dir("NOTES").unwrap();
    }
    let word = |at| u16::from_le_bytes([image[at], image[at + 1]]) as usize;
    let start = (word(14) + usize::from(image[16]) * word(22)) * word(11);
    let root_len = word(17) * 32;
    let masked = image[start..start + root_len]
        .as_chunks_mut::<32>()
        .0
        .iter_mut()
        .find(|entry| &entry[..11] == b"MASKED  DAT")
        .unwrap();
    masked[11] = attributes::HIDDEN | attributes::SYSTEM;
    masked[22..24].copy_from_slice(&0x7883u16.to_le_bytes());
    masked[24..26].copy_from_slice(&0x285du16.to_le_bytes());
    let path = scratch.path().join("attribute fixture.img");
    std::fs::write(&path, image).unwrap();
    let mount = Mount::open_fat_image(&path).unwrap();
    assert_eq!(
        mount
            .metadata("MASKED.DAT")
            .unwrap()
            .modified
            .unwrap()
            .packed(),
        (0x285d, 0x7883)
    );

    // FindFirst hides the entry for CX=0, returns it for CX=6, and puts both
    // flags in the default DTA. Get Attributes returns the same bits in CX.
    let mut probe = vec![
        0xba, 0, 2, 0x31, 0xc9, 0xb4, 0x4e, 0xcd, 0x21, 0x72, 5, 0xb8, 1, 0x4c, 0xcd, 0x21, 0xba,
        0, 2, 0xb9, 6, 0, 0xb4, 0x4e, 0xcd, 0x21, 0x73, 5, 0xb8, 2, 0x4c, 0xcd, 0x21, 0xa0, 0x95,
        0, 0x24, 6, 0x3c, 6, 0x74, 5, 0xb8, 3, 0x4c, 0xcd, 0x21, 0xba, 0, 2, 0xb8, 0, 0x43, 0xcd,
        0x21, 0x73, 5, 0xb8, 4, 0x4c, 0xcd, 0x21, 0x80, 0xe1, 6, 0x80, 0xf9, 6, 0x74, 5, 0xb8, 5,
        0x4c, 0xcd, 0x21, 0xb8, 0, 0x4c, 0xcd, 0x21,
    ];
    probe.resize(0x100, 0);
    probe.extend_from_slice(b"A:\\MASKED.DAT\0");
    std::fs::write(scratch.path().join("PROBE.COM"), probe).unwrap();

    let a = format!("A={}", path.display());
    let c = format!("C={}", scratch.path().display());
    for (command, expected) in [
        ("DIR A:\\ /B/O:N", &b"NORMAL.TXT\r\nNOTES\r\n"[..]),
        (
            "DIR A:\\ /B/A",
            &b"NORMAL.TXT\r\nMASKED.DAT\r\nNOTES\r\n"[..],
        ),
        ("DIR A:\\ /B/A:HS-R", &b""[..]),
        ("DIR A:\\ /B/A:HSA", &b""[..]),
        ("DIR A:\\ /B/A:HS-AD", &b""[..]),
        ("DIR A:\\ /B/A:HS-A-D", &b"MASKED.DAT\r\n"[..]),
        (
            "DIR A:\\ /B/A:R-D/O:N",
            &b"MASKED.DAT\r\nNORMAL.TXT\r\n"[..],
        ),
        ("C:\\PROBE.COM", &b""[..]),
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .args([
                "--headless",
                "--mount-fat",
                &a,
                "--mount",
                &c,
                "--exec",
                command,
            ])
            .output()
            .unwrap();
        assert!(result.status.success(), "{command}: {result:?}");
        assert_eq!(result.stdout, expected, "{command}");
    }
}

#[test]
fn rejects_a_truncated_image() {
    let file = tempfile::NamedTempFile::new().unwrap();
    assert!(dosa_mount::Mount::open_fat_image(file.path()).is_err());
}

fn partitioned_image() -> Vec<u8> {
    const SECTOR: usize = 512;
    const START: usize = 63;
    const LENGTH: usize = 16_384;
    let mut disk = vec![0; (START + LENGTH + 8) * SECTOR];
    disk[510..512].copy_from_slice(&[0x55, 0xaa]);
    disk[446 + 4] = 0x07; // A non-FAT primary partition before the FAT volume.
    disk[446 + 8..446 + 12].copy_from_slice(&1u32.to_le_bytes());
    disk[446 + 12..446 + 16].copy_from_slice(&8u32.to_le_bytes());
    disk[462] = 0x80;
    disk[462 + 4] = 0x06;
    disk[462 + 8..462 + 12].copy_from_slice(&(START as u32).to_le_bytes());
    disk[462 + 12..462 + 16].copy_from_slice(&(LENGTH as u32).to_le_bytes());
    format_test_volume(
        &mut disk[START * SECTOR..(START + LENGTH) * SECTOR],
        fatfs::FatType::Fat16,
        b"Partitioned FAT16\r\n",
    );
    disk
}

fn format_test_volume(bytes: &mut [u8], kind: fatfs::FatType, contents: &[u8]) {
    let mut fat = Cursor::new(bytes);
    fatfs::format_volume(&mut fat, fatfs::FormatVolumeOptions::new().fat_type(kind)).unwrap();
    fat.rewind().unwrap();
    let volume = fatfs::FileSystem::new(&mut fat, fatfs::FsOptions::new()).unwrap();
    volume
        .root_dir()
        .create_file("HELLO.TXT")
        .unwrap()
        .write_all(contents)
        .unwrap();
    volume.unmount().unwrap();
}

#[test]
fn primary_fat_partition_mounts_through_cli_profile_and_desktop() {
    let scratch = tempfile::tempdir().unwrap();
    let disk = partitioned_image();
    let path = scratch.path().join("DOS disk.img");
    std::fs::write(&path, &disk).unwrap();
    let profile = scratch.path().join("disk.toml");
    std::fs::write(
        &profile,
        "exec = 'TYPE C:\\HELLO.TXT'\n[[mounts]]\ndrive = 'C'\npath = 'DOS disk.img'\nfat_image = true\n",
    )
    .unwrap();
    let mount = format!("C={}", path.display());
    for args in [
        vec!["--mount-fat", &mount, "--exec", r"TYPE C:\HELLO.TXT"],
        vec!["--profile", profile.to_str().unwrap()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--headless")
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"Partitioned FAT16\r\n");
    }
    let mut shell = Shell::new();
    shell
        .execute(&format!("MOUNT C /FAT \"{}\"", path.display()))
        .unwrap();
    assert_eq!(
        shell.machine.drive('C').unwrap().read("hello.txt").unwrap(),
        b"Partitioned FAT16\r\n"
    );
    let mut desktop = Desktop::new(Shell::new(), None);
    desktop.menu_action(MenuAction::Mount {
        drive: 'C',
        source: path.clone(),
        kind: MountKind::Fat,
    });
    assert_eq!(desktop.menu().status, "Mounted C:");
    assert_eq!(
        desktop.menu().drives.get(&'C'),
        Some(&"FAT image (read-only)")
    );
    assert_eq!(std::fs::read(path).unwrap(), disk);
}

#[test]
fn rejects_partition_and_volume_ranges_that_exceed_their_container() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("broken.img");
    let mut disk = partitioned_image();
    let good_length = disk.len();
    // The containing file still holds the whole FAT, but the partition is smaller.
    disk[474..478].copy_from_slice(&100u32.to_le_bytes());
    std::fs::write(&path, &disk).unwrap();
    let error = Mount::open_fat_image(&path).unwrap_err();
    assert!(error.to_string().contains("FAT volume geometry"), "{error}");

    disk[474..478].copy_from_slice(&16_384u32.to_le_bytes());
    std::fs::write(&path, &disk[..good_length - 9 * 512]).unwrap();
    let error = Mount::open_fat_image(&path).unwrap_err();
    assert!(
        error.to_string().contains("truncated MBR partition"),
        "{error}"
    );

    disk[458..462].copy_from_slice(&64u32.to_le_bytes());
    std::fs::write(&path, &disk).unwrap();
    let error = Mount::open_fat_image(&path).unwrap_err();
    assert!(
        error.to_string().contains("overlapping MBR partitions"),
        "{error}"
    );
}

const EXTENDED_BASE: usize = 3000;
const SECOND_EBR: usize = EXTENDED_BASE + 16;
const THIRD_EBR: usize = EXTENDED_BASE + 48;
const LOGICAL_LENGTH: usize = 16_384;

fn write_partition(
    disk: &mut [u8],
    table: usize,
    slot: usize,
    kind: u8,
    start: usize,
    sectors: usize,
) {
    let base = table * 512;
    disk[base + 510..base + 512].copy_from_slice(&[0x55, 0xaa]);
    let entry = &mut disk[base + 446 + slot * 16..][..16];
    entry[4] = kind;
    entry[8..12].copy_from_slice(&(start as u32).to_le_bytes());
    entry[12..16].copy_from_slice(&(sectors as u32).to_le_bytes());
}

fn logical_image() -> Vec<u8> {
    let end = THIRD_EBR + 1 + LOGICAL_LENGTH;
    let mut disk = vec![0; (end + 8) * 512];
    write_partition(&mut disk, 0, 1, 0x0f, EXTENDED_BASE, end - EXTENDED_BASE);
    for (table, next) in [(EXTENDED_BASE, SECOND_EBR), (SECOND_EBR, THIRD_EBR)] {
        write_partition(&mut disk, table, 0, 0x07, 1, 8);
        write_partition(&mut disk, table, 1, 0x0f, next - EXTENDED_BASE, end - next);
    }
    write_partition(&mut disk, THIRD_EBR, 0, 0x06, 1, LOGICAL_LENGTH);
    format_test_volume(
        &mut disk[(THIRD_EBR + 1) * 512..end * 512],
        fatfs::FatType::Fat16,
        b"Logical FAT16\r\n",
    );
    disk
}

#[test]
fn walks_ebr_links_and_selects_partitions_from_cli_and_profile() {
    let scratch = tempfile::tempdir().unwrap();
    let image = scratch.path().join("mixed image.img");
    let mut disk = logical_image();
    std::fs::write(&image, &disk).unwrap();
    let auto = Mount::open_fat_image(&image).unwrap();
    assert_eq!(auto.read("HELLO.TXT").unwrap(), b"Logical FAT16\r\n");
    drop(auto);

    write_partition(&mut disk, 0, 0, 0x01, 1, 2880);
    format_test_volume(
        &mut disk[512..2881 * 512],
        fatfs::FatType::Fat12,
        b"Primary FAT12\r\n",
    );
    std::fs::write(&image, &disk).unwrap();
    let auto = Mount::open_fat_image(&image).unwrap();
    assert_eq!(auto.read("HELLO.TXT").unwrap(), b"Primary FAT12\r\n");
    let primary = Mount::open_fat_partition(&image, 1).unwrap();
    assert_eq!(primary.read("HELLO.TXT").unwrap(), b"Primary FAT12\r\n");
    assert!(Mount::open_fat_partition(&image, 6).is_err());

    let profile = scratch.path().join("disk.toml");
    std::fs::write(&profile, "exec = 'type hello.txt'\n[[mounts]]\ndrive = 'C'\npath = 'mixed image.img'\nfat_image = true\nfat_partition = 7\n").unwrap();
    for args in [
        vec![
            "--mount-fat",
            &format!("C:7={}", image.display()),
            "--exec",
            "type hello.txt",
        ],
        vec!["--profile", profile.to_str().unwrap()],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_dosa"))
            .arg("--headless")
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, b"Logical FAT16\r\n");
    }
    assert_eq!(std::fs::read(image).unwrap(), disk);
}

#[test]
fn rejects_backward_and_out_of_bounds_extended_partition_links() {
    let scratch = tempfile::tempdir().unwrap();
    let image = scratch.path().join("broken.img");
    let mut disk = logical_image();
    for (link_lba, expected) in [
        (SECOND_EBR - EXTENDED_BASE, "backward or overlapping"),
        (disk.len() / 512, "invalid extended boot record link"),
    ] {
        write_partition(&mut disk, SECOND_EBR, 1, 0x0f, link_lba, 1);
        std::fs::write(&image, &disk).unwrap();
        let error = Mount::open_fat_image(&image).unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}
