use std::fs::FileTimes;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, Metadata, OpenOptions};

use super::{
    DosTimestamp, Entry, MAX_FILE_BYTES, Mount, MountError, Result, Source, host_children, lock,
    read_bounded,
};
use crate::names::{canonical_name, find_child, guest_components};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(super) struct WritableDirectory {
    pub(super) root: Arc<Dir>,
    transaction: Arc<Mutex<()>>,
    pub(super) session: Option<Arc<tempfile::TempDir>>,
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
    writable: bool,
    _session: Option<Arc<tempfile::TempDir>>,
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
        let (file, length) = self.open_reader()?;
        read_bounded(file, length)
    }

    /// Read at most the requested bytes without buffering the rest of the file.
    pub fn read_at(&self, position: u64, length: usize) -> Result<Vec<u8>> {
        let _guard = lock(&self.transaction)?;
        let (mut file, size) = self.open_reader()?;
        let limit = size.saturating_sub(position).min(length as u64);
        if limit == 0 {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(position))?;
        let mut bytes = Vec::new();
        file.take(limit).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub fn modified(&self) -> Result<Option<DosTimestamp>> {
        let _guard = lock(&self.transaction)?;
        let metadata = self.open_parent()?.symlink_metadata(&self.name)?;
        if !metadata.is_file() {
            return Err(MountError::UnsupportedSource);
        }
        Ok(metadata
            .modified()
            .ok()
            .and_then(|time| DosTimestamp::from_host(time.into_std())))
    }

    pub fn length(&self) -> Result<u64> {
        let _guard = lock(&self.transaction)?;
        self.open_reader().map(|(_, length)| length)
    }

    fn open_reader(&self) -> Result<(File, u64)> {
        let parent = self.open_parent()?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = parent.open_with(&self.name, &options)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(MountError::UnsupportedSource);
        }
        if metadata.len() > MAX_FILE_BYTES {
            return Err(MountError::FileTooLarge);
        }
        Ok((file, metadata.len()))
    }

    /// Atomically replace the file's contents inside its host directory.
    pub fn replace(&self, bytes: &[u8]) -> Result<()> {
        self.replace_at(bytes, None)
    }

    /// Atomically replace the file with an optional DOS modification time.
    pub fn replace_at(&self, bytes: &[u8], modified: Option<SystemTime>) -> Result<()> {
        if !self.writable {
            return Err(MountError::ReadOnly);
        }
        let _guard = lock(&self.transaction)?;
        let parent = self.open_parent()?;
        self.replace_locked(&parent, bytes, modified)
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

    fn replace_locked(
        &self,
        parent: &Dir,
        bytes: &[u8],
        modified: Option<SystemTime>,
    ) -> Result<()> {
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
                set_modified(&file, modified)?;
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

fn set_modified(file: &File, modified: Option<SystemTime>) -> io::Result<()> {
    if let Some(time) = modified {
        file.try_clone()?
            .into_std()
            .set_times(FileTimes::new().set_modified(time))?;
    }
    Ok(())
}

impl Mount {
    /// Open a live read-only handle in a directory with explicitly enabled writes.
    pub fn open_readable_file(&self, path: &str) -> Result<WritableFile> {
        let directory = self.writable()?;
        let _guard = lock(&directory.transaction)?;
        let target = resolve_target(&directory.root, path)?;
        let name = target.file_name(path)?;
        let file = WritableFile {
            root: Arc::clone(&directory.root),
            parents: target.parents,
            name,
            transaction: Arc::clone(&directory.transaction),
            writable: false,
            _session: directory.session.clone(),
        };
        file.open_reader()?;
        Ok(file)
    }

    /// Open a host directory with permission to create, replace, and remove guest files.
    pub fn open_writable_directory(path: impl AsRef<Path>) -> Result<Self> {
        let root = Dir::open_ambient_dir(path, ambient_authority())?;
        Ok(Self::writable_directory(root, None))
    }

    /// Mount a writable, temporary copy of the DOS-visible files in a ZIP archive.
    pub fn open_session_zip(path: impl AsRef<Path>) -> Result<Self> {
        Self::open(path)?.writable_zip_copy()
    }

    pub fn is_read_only_zip(&self) -> bool {
        matches!(self.source, Source::Zip(_))
    }

    /// Copy an already-mounted ZIP into a writable session using its open archive.
    pub fn writable_zip_copy(&self) -> Result<Self> {
        let Source::Zip(zip) = &self.source else {
            return Err(MountError::ExpectedZip);
        };
        let session = tempfile::tempdir()?;
        let root = Dir::open_ambient_dir(session.path(), ambient_authority())?;
        zip.copy_to(&root)?;
        Ok(Self::writable_directory(root, Some(Arc::new(session))))
    }

    fn writable_directory(root: Dir, session: Option<Arc<tempfile::TempDir>>) -> Self {
        Self {
            source: Source::WritableDirectory(WritableDirectory {
                root: Arc::new(root),
                transaction: Arc::new(Mutex::new(())),
                session,
            }),
        }
    }

    pub fn is_writable(&self) -> bool {
        matches!(self.source, Source::WritableDirectory(_))
    }

    /// Report the effective guest read-only flag for an existing path.
    pub fn is_readonly(&self, path: &str) -> Result<bool> {
        if self.metadata(path)?.is_dir {
            return Ok(false);
        }
        if !self.is_writable() {
            return Ok(true);
        }
        let directory = self.writable()?;
        let _guard = lock(&directory.transaction)?;
        let target = resolve_target(&directory.root, path)?;
        let entry = target
            .existing
            .ok_or_else(|| MountError::NotFound(path.to_owned()))?;
        Ok(target
            .parent
            .symlink_metadata(entry.host_name)?
            .permissions()
            .readonly())
    }

    pub fn open_writable_file(
        &self,
        path: &str,
        disposition: WriteDisposition,
    ) -> Result<WritableFile> {
        self.open_writable_file_at(path, disposition, None)
    }

    /// Create or truncate a file with an optional DOS modification time.
    pub fn create_file(&self, path: &str, modified: Option<SystemTime>) -> Result<WritableFile> {
        self.open_writable_file_at(path, WriteDisposition::CreateOrTruncate, modified)
    }

    fn open_writable_file_at(
        &self,
        path: &str,
        disposition: WriteDisposition,
        modified: Option<SystemTime>,
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
            writable: true,
            _session: writable.session.clone(),
        };
        if exists {
            if create {
                file.replace_locked(&target.parent, &[], modified)?;
            } else {
                writable_file_metadata(&target.parent, &file.name)?;
            }
        } else {
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            let created = target.parent.open_with(&file.name, &options)?;
            if let Err(error) = set_modified(&created, modified) {
                drop(created);
                let _ = target.parent.remove_file(&file.name);
                return Err(error.into());
            }
        }
        Ok(file)
    }

    /// Copy completed data into a new guest file without replacing an existing entry.
    pub fn write_new_file(&self, path: &str, bytes: &[u8]) -> Result<()> {
        self.write_new_file_at(path, bytes, None)
    }

    /// Copy completed data and an optional DOS modification time into a new file.
    pub fn write_new_file_at(
        &self,
        path: &str,
        bytes: &[u8],
        modified: Option<SystemTime>,
    ) -> Result<()> {
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(MountError::FileTooLarge);
        }
        let writable = self.writable()?;
        let _guard = lock(&writable.transaction)?;
        let target = resolve_target(&writable.root, path)?;
        let name = target.new_name(path)?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let mut file = target.parent.open_with(&name, &options)?;
        let result = (|| {
            file.write_all(bytes)?;
            set_modified(&file, modified)?;
            file.sync_all()
        })();
        if let Err(error) = result {
            // Release the handle before removing the incomplete file on Windows.
            drop(file);
            let _ = target.parent.remove_file(&name);
            return Err(error.into());
        }
        Ok(())
    }

    pub fn remove_file(&self, path: &str) -> Result<()> {
        self.remove_file_with_name(path, None)
    }

    /// Remove a listed file only if the guest path still resolves to its original host name.
    pub fn remove_listed_file(
        &self,
        path: &str,
        parents: &[String],
        host_name: &str,
    ) -> Result<()> {
        self.remove_file_with_name(path, Some((parents, host_name)))
    }

    fn remove_file_with_name(&self, path: &str, expected: Option<(&[String], &str)>) -> Result<()> {
        let writable = self.writable()?;
        let _guard = lock(&writable.transaction)?;
        let target = resolve_target(&writable.root, path)?;
        let name = target.file_name(path)?;
        writable_file_metadata(&target.parent, &name)?;
        if expected.is_some_and(|(parents, expected_name)| {
            parents != target.parents || expected_name != name
        }) {
            return Err(MountError::NotFound(path.to_owned()));
        }
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
