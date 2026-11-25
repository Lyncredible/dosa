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
    let mut fat = Cursor::new(&mut disk[START * SECTOR..(START + LENGTH) * SECTOR]);
    fatfs::format_volume(
        &mut fat,
        fatfs::FormatVolumeOptions::new().fat_type(fatfs::FatType::Fat16),
    )
    .unwrap();
    fat.rewind().unwrap();
    let volume = fatfs::FileSystem::new(&mut fat, fatfs::FsOptions::new()).unwrap();
    volume
        .root_dir()
        .create_file("HELLO.TXT")
        .unwrap()
        .write_all(b"Partitioned FAT16\r\n")
        .unwrap();
    volume.unmount().unwrap();
    disk
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
