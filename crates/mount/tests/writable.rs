use std::{fs, io};

use dosa_mount::{Mount, MountError, WriteDisposition::*};

#[test]
fn writable_directory_persists_replacements_and_directory_mutations() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let mount = Mount::open_writable_directory(root).unwrap();
    assert!(mount.is_writable());

    mount.create_dir("saved").unwrap();
    let file = mount
        .open_writable_file("saved/level.sav", CreateOrTruncate)
        .unwrap();
    file.replace(b"first save").unwrap();
    assert_eq!(file.read().unwrap(), b"first save");
    assert_eq!(file.length().unwrap(), 10);
    assert_eq!(file.read_at(6, 4).unwrap(), b"save");
    assert_eq!(file.read_at(8, 10).unwrap(), b"ve");
    assert!(file.read_at(100, 10).unwrap().is_empty());
    assert_eq!(mount.read("SAVED/LEVEL.SAV").unwrap(), b"first save");
    assert!(mount.remove_dir("SAVED").is_err());

    let other = mount
        .open_writable_file("second.sav", CreateOrTruncate)
        .unwrap();
    other.replace(b"keep this").unwrap();
    drop(other);
    assert!(matches!(
        mount.rename_file("saved/level.sav", "second.sav"),
        Err(MountError::AlreadyExists(_))
    ));
    assert_eq!(mount.read("second.sav").unwrap(), b"keep this");

    drop(file);
    mount.rename_file("saved/level.sav", "phase1.dat").unwrap();
    mount.remove_dir("saved").unwrap();
    assert!(!root.join("SAVED").exists());
    drop(mount);

    let restored = Mount::open_writable_directory(root).unwrap();
    assert_eq!(restored.read("phase1.dat").unwrap(), b"first save");
    restored.remove_file("phase1.dat").unwrap();
    assert!(restored.metadata("phase1.dat").unwrap_err().is_not_found());
}

#[test]
fn default_mount_cannot_mutate_even_when_host_area_is_writable() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    fs::write(root.join("KEEP.DAT"), b"original").unwrap();
    let mount = Mount::open(root).unwrap();
    assert!(!mount.is_writable());
    assert!(matches!(
        mount.open_writable_file("keep.dat", CreateOrTruncate),
        Err(MountError::ReadOnly)
    ));
    assert!(matches!(
        mount.remove_file("keep.dat"),
        Err(MountError::ReadOnly)
    ));
    assert!(matches!(
        mount.create_dir("saves"),
        Err(MountError::ReadOnly)
    ));
    assert_eq!(fs::read(root.join("KEEP.DAT")).unwrap(), b"original");
    assert!(Mount::open_writable_directory(root.join("KEEP.DAT")).is_err());
}

#[test]
fn live_read_handle_accepts_readonly_host_file_but_rejects_replacement() {
    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("READONLY.DAT");
    fs::write(&path, b"readable").unwrap();
    let original_permissions = fs::metadata(&path).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let mount = Mount::open_writable_directory(scratch.path()).unwrap();
    assert!(mount.is_readonly("READONLY.DAT").unwrap());
    let handle = mount.open_readable_file("READONLY.DAT").unwrap();
    assert_eq!(handle.read_at(0, 8).unwrap(), b"readable");
    assert!(matches!(
        handle.replace(b"changed"),
        Err(MountError::ReadOnly)
    ));
    assert_eq!(fs::read(&path).unwrap(), b"readable");
    drop(handle);
    drop(mount);
    fs::set_permissions(&path, original_permissions).unwrap();
}

#[test]
fn replacing_a_writable_file_detaches_existing_hard_links() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("mount");
    fs::create_dir(&root).unwrap();
    let outside = scratch.path().join("outside.dat");
    fs::write(&outside, b"outside data").unwrap();
    fs::hard_link(&outside, root.join("LINK.DAT")).unwrap();

    let mount = Mount::open_writable_directory(&root).unwrap();
    let file = mount.open_writable_file("link.dat", OpenExisting).unwrap();
    file.replace(b"inside data").unwrap();
    assert_eq!(fs::read(&outside).unwrap(), b"outside data");
    assert_eq!(fs::read(root.join("LINK.DAT")).unwrap(), b"inside data");
    drop(file);

    assert!(
        mount
            .open_writable_file("../outside.dat", CreateOrTruncate)
            .is_err()
    );
    assert!(mount.rename_file("link.dat", "../outside.dat").is_err());
    assert!(mount.open_writable_file("NUL", CreateOrTruncate).is_err());
    assert!(
        mount
            .open_writable_file("CONIN$", CreateOrTruncate)
            .is_err()
    );
    assert_eq!(fs::read(&outside).unwrap(), b"outside data");
}

#[test]
fn writable_mount_rejects_symlink_parents_and_targets() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("mount");
    let outside = scratch.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("KEEP.DAT"), b"original").unwrap();

    #[cfg(windows)]
    let symlink = std::os::windows::fs::symlink_dir(&outside, root.join("OTHER"));
    #[cfg(unix)]
    let symlink = std::os::unix::fs::symlink(&outside, root.join("OTHER"));
    #[cfg(not(any(windows, unix)))]
    let symlink: io::Result<()> = Err(io::ErrorKind::Unsupported.into());

    if let Err(error) = symlink {
        if matches!(
            error.kind(),
            io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
        ) || error.raw_os_error() == Some(1314)
        {
            return;
        }
        panic!("{error}");
    }
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(outside.join("KEEP.DAT"), root.join("LINK.DAT")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("KEEP.DAT"), root.join("LINK.DAT")).unwrap();

    let mount = Mount::open_writable_directory(&root).unwrap();
    assert!(
        mount
            .open_writable_file("OTHER/KEEP.DAT", CreateOrTruncate)
            .is_err()
    );
    assert!(
        mount
            .open_writable_file("LINK.DAT", CreateOrTruncate)
            .is_err()
    );
    assert!(mount.remove_dir("OTHER").is_err());
    assert!(mount.remove_file("LINK.DAT").is_err());
    assert_eq!(fs::read(outside.join("KEEP.DAT")).unwrap(), b"original");
    assert!(
        fs::symlink_metadata(root.join("LINK.DAT"))
            .unwrap()
            .is_symlink()
    );
}

#[test]
fn opened_writable_file_cannot_follow_its_parent_outside_the_mount() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().join("mount");
    let nested = root.join("SUB");
    let outside = scratch.path().join("other");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("DATA.DAT"), b"keep this").unwrap();
    let mount = Mount::open_writable_directory(&root).unwrap();
    let file = mount
        .open_writable_file("sub/data.dat", OpenExisting)
        .unwrap();

    fs::rename(&nested, &outside).unwrap();
    assert!(file.replace(b"must fail").is_err());
    assert_eq!(fs::read(outside.join("DATA.DAT")).unwrap(), b"keep this");
}
