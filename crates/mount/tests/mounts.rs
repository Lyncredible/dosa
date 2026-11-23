use dosa_mount::{Mount, MountError};
use std::{
    fs::{self, File},
    io::Write,
};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

#[test]
fn directory_mount_lists_and_opens_case_insensitive_short_names() {
    let temp = tempdir().unwrap();
    let folder = temp.path().join("Assets");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("LONGNA~1.EXE"), b"existing").unwrap();
    fs::write(folder.join("Long Named Game.exe"), b"long").unwrap();

    let mount = Mount::open(temp.path()).unwrap();
    assert!(mount.metadata(r"\aSsets").unwrap().is_dir);
    let entries = mount.list("/assets").unwrap();
    let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["LONGNA~1.EXE", "LONGNA~2.EXE"]);
    assert_eq!(entries[1].host_name, "Long Named Game.exe");
    assert_eq!(entries[1].size, 4);
    assert_eq!(mount.read(r"assets\longna~2.exe").unwrap(), b"long");
    assert_eq!(mount.read("assets/long named game.exe").unwrap(), b"long");
    assert_eq!(mount.read("assets/longna~1.exe").unwrap(), b"existing");
    assert!(mount.read("missing").unwrap_err().is_not_found());
    assert!(matches!(
        mount.read("assets"),
        Err(MountError::IsDirectory(_))
    ));
    assert!(matches!(
        mount.list(r"assets\longna~1.exe"),
        Err(MountError::NotDirectory(_))
    ));
}

#[test]
fn file_mount_only_exposes_the_selected_file() {
    let temp = tempdir().unwrap();
    let file = temp.path().join("hello.com");
    fs::write(&file, b"program").unwrap();
    fs::write(temp.path().join("other.com"), b"other").unwrap();
    let mount = Mount::open(file).unwrap();

    assert_eq!(mount.list("").unwrap()[0].name, "HELLO.COM");
    assert_eq!(mount.list("").unwrap().len(), 1);
    assert_eq!(mount.read("HELLO.COM").unwrap(), b"program");
    assert_eq!(mount.read("hello.com").unwrap(), b"program");
    assert!(mount.read("other.com").unwrap_err().is_not_found());
}

#[test]
fn zip_mount_reads_deflated_files_and_lists_nested_and_empty_directories() {
    let temp = tempdir().unwrap();
    let file = temp.path().join("fixture.ZIP");
    let mut zip = ZipWriter::new(File::create(&file).unwrap());
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = stored.compression_method(CompressionMethod::Deflated);
    zip.start_file("Long Folder/Readme.txt", deflated).unwrap();
    zip.write_all(b"text inside archive").unwrap();
    zip.add_directory("Long Folder/", stored).unwrap();
    zip.add_directory("Empty/", stored).unwrap();
    zip.add_symlink("alias", "Long Folder/Readme.txt", stored)
        .unwrap();
    zip.finish().unwrap();

    let mount = Mount::open(file).unwrap();
    assert_eq!(mount.list("").unwrap().len(), 2);
    assert!(mount.metadata("long folder").unwrap().is_dir);
    assert_eq!(mount.list(r"\LONGFO~1").unwrap()[0].name, "README.TXT");
    assert_eq!(
        mount.read(r"\longfo~1\readme.txt").unwrap(),
        b"text inside archive"
    );
    assert!(mount.list("EMPTY").unwrap().is_empty());
    assert!(mount.read("alias").unwrap_err().is_not_found());
}

#[test]
fn drive_paths_reject_traversal_above_the_root() {
    let temp = tempdir().unwrap();
    fs::write(temp.path().join("root.txt"), b"root").unwrap();
    let mount = Mount::open(temp.path()).unwrap();
    for path in [r"..\root.txt", r"/Games/../../root.txt", r"C:\root.txt"] {
        assert!(matches!(mount.read(path), Err(MountError::InvalidPath(_))));
    }
    assert_eq!(mount.read(r"\.\root.txt").unwrap(), b"root");
}

#[cfg(any(unix, windows))]
#[test]
fn directory_mount_skips_symbolic_links() {
    let drive = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let file = outside.path().join("other.txt");
    fs::write(&file, b"outside").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&file, drive.path().join("other.txt")).unwrap();
        std::os::unix::fs::symlink(outside.path(), drive.path().join("other")).unwrap();
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        match symlink_file(&file, drive.path().join("other.txt")) {
            // Windows can require developer mode or the symbolic-link privilege.
            Err(error) if error.raw_os_error() == Some(1314) => return,
            result => result.unwrap(),
        }
        symlink_dir(outside.path(), drive.path().join("other")).unwrap();
    }
    let mount = Mount::open(drive.path()).unwrap();

    assert!(mount.list("").unwrap().is_empty());
    assert!(mount.read("other.txt").unwrap_err().is_not_found());
    assert!(mount.read("other/other.txt").unwrap_err().is_not_found());
}

#[test]
fn oversized_file_is_rejected_from_its_metadata() {
    let temp = tempdir().unwrap();
    let file = temp.path().join("big.dat");
    File::create(&file)
        .unwrap()
        .set_len(256 * 1024 * 1024 + 1)
        .unwrap();
    let mount = Mount::open(file).unwrap();
    assert!(matches!(
        mount.read("big.dat"),
        Err(MountError::FileTooLarge)
    ));
}
