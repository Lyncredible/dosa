use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata, OpenOptions};

use super::{
    Entry, MAX_FILE_BYTES, Mount, MountError, Result, Source, host_children, lock, read_bounded,
};
use crate::names::{canonical_name, find_child, guest_components};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct WritableDirectory {
    pub(super) root: Arc<Dir>,
    transaction: Arc<Mutex<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteDisposition {
    OpenExisting,
    CreateOrTruncate,
}

/// A file in an explicitly writable directory.
///
/// A write replaces the directory entry with a freshly staged file. This
/// leaves other hard links to the original file's contents intact.
#[derive(Debug)]
pub struct WritableFile {
    root: Arc<Dir>,
    parents: Vec<String>,
    name: String,
    transaction: Arc<Mutex<()>>,
}

impl WritableFile {
    /// Whether two handles opened through the same mount address the same file.
    pub fn same_target(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.root, &other.root)
            && self.parents == other.parents
            && self.name == other.name
    }

    /// Read the current contents using the drive's normal file size limit.
    pub fn read(&self) -> Result<Vec<u8>> {
        let _guard = lock(&self.transaction)?;
        let parent = self.open_parent()?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = parent.open_with(&self.name, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(MountError::UnsupportedSource);
        }
        read_bounded(file, metadata.len())
    }

    /// Atomically replace the file's contents inside its host directory.
    pub fn replace(&self, bytes: &[u8]) -> Result<()> {
        let _guard = lock(&self.transaction)?;
        let parent = self.open_parent()?;
        self.replace_locked(&parent, bytes)
    }

    fn open_parent(&self) -> Result<Dir> {
        // Reopen every parent from the original root. A child directory
        // moved out of the mount after the file was opened goes out of scope.
        let mut parent = self.root.try_clone()?;
        for name in &self.parents {
            parent = parent.open_dir_nofollow(name)?;
        }
        Ok(parent)
    }

    fn replace_locked(&self, parent: &Dir, bytes: &[u8]) -> Result<()> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(MountError::FileTooLarge);
        }
        let metadata = writable_file_metadata(parent, &self.name)?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        for _ in 0..32 {
            let serial = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let temp = format!(".dosa-write-{}-{serial}", std::process::id());
            let mut file = match parent.open_with(&temp, &options) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            };
            let result = (|| {
                file.write_all(bytes)?;
                file.set_permissions(metadata.permissions())?;
                file.sync_all()
            })();
            // Windows does not permit the staging file to be renamed while its
            // handle is still open without delete sharing.
            drop(file);
            let result = result.and_then(|()| parent.rename(&temp, parent, &self.name));
            if let Err(error) = result {
                let _ = parent.remove_file(&temp);
                return Err(error.into());
            }
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a staging file",
        )
        .into())
    }
}

impl Mount {
    /// Open a host directory with permission to create, replace, and remove guest files.
    pub fn open_writable_directory(path: impl AsRef<Path>) -> Result<Self> {
        let root = Dir::open_ambient_dir(path, ambient_authority())?;
        Ok(Self {
            source: Source::WritableDirectory(WritableDirectory {
                root: Arc::new(root),
                transaction: Arc::new(Mutex::new(())),
            }),
        })
    }

    pub fn is_writable(&self) -> bool {
        matches!(self.source, Source::WritableDirectory(_))
    }

    pub fn open_writable_file(
        &self,
        path: &str,
        disposition: WriteDisposition,
    ) -> Result<WritableFile> {
        let writable = self.writable()?;
        let _guard = lock(&writable.transaction)?;
        let target = resolve_target(&writable.root, path)?;
        let create = disposition == WriteDisposition::CreateOrTruncate;
        let exists = target.existing.is_some();
        let name = if exists {
            target.file_name(path)?
        } else if create {
            target.new_name(path)?
        } else {
            return Err(MountError::NotFound(path.to_owned()));
        };

        let file = WritableFile {
            root: Arc::clone(&writable.root),
            parents: target.parents,
            name,
            transaction: Arc::clone(&writable.transaction),
        };
        if exists {
            if create {
                file.replace_locked(&target.parent, &[])?;
            } else {
                writable_file_metadata(&target.parent, &file.name)?;
            }
        } else {
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            target.parent.open_with(&file.name, &options)?;
        }
        Ok(file)
    }

    pub fn remove_file(&self, path: &str) -> Result<()> {
        let writable = self.writable()?;
        let _guard = lock(&writable.transaction)?;
        let target = resolve_target(&writable.root, path)?;
        let name = target.file_name(path)?;
        writable_file_metadata(&target.parent, &name)?;
        target.parent.remove_file(name)?;
        Ok(())
    }

    pub fn rename_file(&self, from: &str, to: &str) -> Result<()> {
        let writable = self.writable()?;
        let _guard = lock(&writable.transaction)?;
        let source = resolve_target(&writable.root, from)?;
        let from_name = source.file_name(from)?;
        writable_file_metadata(&source.parent, &from_name)?;
        let target = resolve_target(&writable.root, to)?;
        let new_name = target.new_name(to)?;
        source.parent.rename(from_name, &target.parent, new_name)?;
        Ok(())
    }

    pub fn create_dir(&self, path: &str) -> Result<()> {
        let writable = self.writable()?;
        let _guard = lock(&writable.transaction)?;
        let target = resolve_target(&writable.root, path)?;
        target.parent.create_dir(target.new_name(path)?)?;
        Ok(())
    }

    /// Remove only an empty directory.
    pub fn remove_dir(&self, path: &str) -> Result<()> {
        let writable = self.writable()?;
        let _guard = lock(&writable.transaction)?;
        let target = resolve_target(&writable.root, path)?;
        let entry = target
            .existing
            .ok_or_else(|| MountError::NotFound(path.to_owned()))?;
        if !entry.is_dir || !target.parent.symlink_metadata(&entry.host_name)?.is_dir() {
            return Err(MountError::NotDirectory(path.to_owned()));
        }
        target.parent.remove_dir(entry.host_name)?;
        Ok(())
    }

    fn writable(&self) -> Result<&WritableDirectory> {
        match &self.source {
            Source::WritableDirectory(directory) => Ok(directory),
            _ => Err(MountError::ReadOnly),
        }
    }
}

struct Target<'a> {
    parent: Dir,
    parents: Vec<String>,
    requested_name: &'a str,
    existing: Option<Entry>,
}

impl Target<'_> {
    fn file_name(&self, path: &str) -> Result<String> {
        match &self.existing {
            Some(entry) if entry.is_dir => Err(MountError::IsDirectory(path.to_owned())),
            Some(entry) => Ok(entry.host_name.clone()),
            None => Err(MountError::NotFound(path.to_owned())),
        }
    }

    fn new_name(&self, path: &str) -> Result<String> {
        if self.existing.is_some() {
            return Err(MountError::AlreadyExists(path.to_owned()));
        }
        let name = canonical_name(self.requested_name)
            .ok_or_else(|| MountError::InvalidPath(path.to_owned()))?;
        let stem = name.split('.').next().unwrap_or("");
        if matches!(stem, "CONIN$" | "CONOUT$" | "CLOCK$") {
            return Err(MountError::InvalidPath(path.to_owned()));
        }
        // The normal directory listing filters symlinks and other special entries.
        // Check the final spelling directly too, so an operation cannot replace one.
        match self.parent.symlink_metadata(&name) {
            Ok(_) => Err(MountError::AlreadyExists(path.to_owned())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(name),
            Err(error) => Err(error.into()),
        }
    }
}

fn resolve_target<'a>(root: &Dir, path: &'a str) -> Result<Target<'a>> {
    let components = guest_components(path)?;
    let Some((&requested_name, parents)) = components.split_last() else {
        return Err(MountError::InvalidPath(path.to_owned()));
    };
    let mut parent = root.try_clone()?;
    let mut host_parents = Vec::new();
    for component in parents {
        let children = host_children(&parent)?;
        let index = find_child(&children, component, path)?;
        let entry = &children[index].entry;
        if !entry.is_dir {
            return Err(MountError::NotDirectory(path.to_owned()));
        }
        parent = parent.open_dir_nofollow(&entry.host_name)?;
        host_parents.push(entry.host_name.clone());
    }
    let mut children = host_children(&parent)?;
    let existing = match find_child(&children, requested_name, path) {
        Ok(index) => Some(children.swap_remove(index).entry),
        Err(error) if error.is_not_found() => None,
        Err(error) => return Err(error),
    };
    Ok(Target {
        parent,
        parents: host_parents,
        requested_name,
        existing,
    })
}

fn writable_file_metadata(parent: &Dir, name: &str) -> Result<Metadata> {
    let metadata = parent.symlink_metadata(name)?;
    if !metadata.is_file() || metadata.permissions().readonly() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "file is not writable").into());
    }
    Ok(metadata)
}
