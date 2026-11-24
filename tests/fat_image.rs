use std::io::{Seek, Write};
use std::process::Command;

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
