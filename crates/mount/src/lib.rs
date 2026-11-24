//! DOS drives backed by host directories, individual files, or ZIP archives.

mod fat_image;
mod iso_image;
pub use iso_image::ISO_SECTOR_BYTES;
mod names;
mod writable;
mod zip_mount;

use cap_std::{ambient_authority, fs::Dir};
use names::{Child, assign_aliases, find_child, guest_components, valid_component};
use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};
use writable::WritableDirectory;
pub use writable::{WritableFile, WriteDisposition};
use zip_mount::ZipMount;

const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_DEPTH: usize = 64;

/// A directory entry as it appears from DOS.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// The DOS 8.3 name, unique among its siblings.
    pub name: String,
    /// The original filename as it appears in the directory or archive.
    pub host_name: String,
    pub is_dir: bool,
    pub size: u64,
}

impl Entry {
    fn new(host_name: String, is_dir: bool, size: u64) -> Self {
        Self {
            name: String::new(),
            host_name,
            is_dir,
            size,
        }
    }

    fn root() -> Self {
        Self::new(String::new(), true, 0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MountError {
    #[error("invalid DOS path: {0}")]
    InvalidPath(String),
    #[error("DOS path not found: {0}")]
    NotFound(String),
    #[error("DOS path is not a directory: {0}")]
    NotDirectory(String),
    #[error("DOS path is a directory: {0}")]
    IsDirectory(String),
    #[error("DOS path already exists: {0}")]
    AlreadyExists(String),
    #[error("this drive is mounted read-only")]
    ReadOnly,
    #[error("host path is not a regular file or directory")]
    UnsupportedSource,
    #[error("ZIP directory exceeds the mount entry limit")]
    ArchiveTooLarge,
    #[error("writable ZIP copies require a ZIP archive")]
    ExpectedZip,
    #[error("ZIP contents exceed the {limit}-byte writable copy limit")]
    ZipCopyTooLarge { limit: u64 },
    #[error("file exceeds the {MAX_FILE_BYTES}-byte mount read limit")]
    FileTooLarge,
    #[error("mount file lock was poisoned")]
    LockPoisoned,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}

impl MountError {
    /// Whether the guest path was missing when it was opened.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
            || matches!(self, Self::Io(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

type Result<T> = std::result::Result<T, MountError>;

#[derive(Debug)]
enum Source {
    FatImage(fat_image::FatImage),
    IsoImage(iso_image::IsoImage),
    Directory(Arc<Dir>),
    WritableDirectory(WritableDirectory),
    SingleFile(Child<Mutex<File>>),
    Zip(Box<ZipMount>),
}

/// An open host-backed drive. DOS paths use either slash and are relative to the drive root.
#[derive(Debug)]
pub struct Mount {
    source: Source,
}

impl Mount {
    /// Return the declared data-sector count for an optical image mount.
    pub fn iso_sector_count(&self) -> Option<u32> {
        match &self.source {
            Source::IsoImage(image) => Some(image.sector_count()),
            _ => None,
        }
    }

    /// Read cooked 2,048-byte sectors from an optical image.
    pub fn read_iso_sectors(&self, first: u32, bytes: &mut [u8]) -> Result<()> {
        match &self.source {
            Source::IsoImage(image) => image.read_sectors(first, bytes),
            _ => Err(MountError::UnsupportedSource),
        }
    }

    /// Describe the mount source and guest access mode.
    pub fn description(&self) -> &'static str {
        match &self.source {
            Source::Directory(_) => "Folder (read-only)",
            Source::WritableDirectory(directory) if directory.session.is_some() => {
                "ZIP copy (session R/W)"
            }
            Source::WritableDirectory(_) => "Folder (read/write)",
            Source::SingleFile(_) => "File (read-only)",
            Source::Zip(_) => "ZIP (read-only)",
            Source::FatImage(_) => "FAT image (read-only)",
            Source::IsoImage(_) => "ISO image (read-only)",
        }
    }

    /// Mount a FAT volume image as a read-only DOS drive.
    pub fn open_fat_image(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !fs::metadata(path)?.is_file() {
            return Err(MountError::UnsupportedSource);
        }
        let image = fat_image::FatImage::open(File::open(path)?)?;
        Ok(Self {
            source: Source::FatImage(image),
        })
    }

    /// Open the primary ISO 9660 filesystem on a read-only CD image.
    pub fn open_iso_image(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !fs::metadata(path)?.is_file() {
            return Err(MountError::UnsupportedSource);
        }
        Ok(Self {
            source: Source::IsoImage(iso_image::IsoImage::open(File::open(path)?)?),
        })
    }

    /// Open a host directory, a ZIP file (extension matched case-insensitively),
    /// or a single regular file presented in the drive root.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let metadata = fs::metadata(path)?;
        let source = if metadata.is_dir() {
            Source::Directory(Arc::new(Dir::open_ambient_dir(path, ambient_authority())?))
        } else if metadata.is_file() {
            let file = File::open(path)?;
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
            {
                Source::Zip(Box::new(ZipMount::new(file)?))
            } else {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .filter(|name| valid_component(name))
                    .ok_or(MountError::UnsupportedSource)?;
                let entry = Entry::new(name.to_owned(), false, file.metadata()?.len());
                let mut child = Child::new(entry, Mutex::new(file));
                assign_aliases(std::slice::from_mut(&mut child));
                Source::SingleFile(child)
            }
        } else {
            return Err(MountError::UnsupportedSource);
        };
        Ok(Self { source })
    }

    /// Return the DOS entries directly under a guest directory.
    pub fn list(&self, path: &str) -> Result<Vec<Entry>> {
        let components = guest_components(path)?;
        match &self.source {
            Source::Directory(root) | Source::WritableDirectory(WritableDirectory { root, .. }) => {
                if components.is_empty() {
                    return list_host_children(root);
                }
                let (relative, entry) = host_entry(root, &components, path)?;
                if !entry.is_dir {
                    return Err(MountError::NotDirectory(path.to_owned()));
                }
                let dir = root.open_dir(relative)?;
                list_host_children(&dir)
            }
            Source::SingleFile(child) => {
                if components.is_empty() {
                    return Ok(vec![child.entry.clone()]);
                }
                single_entry(child, &components, path)?;
                Err(MountError::NotDirectory(path.to_owned()))
            }
            Source::Zip(zip) => zip.list(&components, path),
            Source::FatImage(image) => image.list(&components, path),
            Source::IsoImage(image) => image.list(&components, path),
        }
    }

    /// Return information about a file or directory.
    pub fn metadata(&self, path: &str) -> Result<Entry> {
        let components = guest_components(path)?;
        if components.is_empty() {
            return Ok(Entry::root());
        }
        match &self.source {
            Source::Directory(root) | Source::WritableDirectory(WritableDirectory { root, .. }) => {
                Ok(host_entry(root, &components, path)?.1)
            }
            Source::SingleFile(child) => {
                single_entry(child, &components, path)?;
                let mut entry = child.entry.clone();
                entry.size = lock(&child.item)?.metadata()?.len();
                Ok(entry)
            }
            Source::Zip(zip) => zip.metadata(&components, path),
            Source::FatImage(image) => image.metadata(&components, path),
            Source::IsoImage(image) => image.metadata(&components, path),
        }
    }

    /// Read a file up to the mount read limit.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let components = guest_components(path)?;
        if components.is_empty() {
            return Err(MountError::IsDirectory(path.to_owned()));
        }
        match &self.source {
            Source::Directory(root) | Source::WritableDirectory(WritableDirectory { root, .. }) => {
                let (relative, entry) = host_entry(root, &components, path)?;
                if entry.is_dir {
                    return Err(MountError::IsDirectory(path.to_owned()));
                }
                let file = root.open(relative)?;
                let metadata = file.metadata()?;
                if !metadata.is_file() {
                    return Err(MountError::UnsupportedSource);
                }
                read_bounded(file, metadata.len())
            }
            Source::SingleFile(child) => {
                single_entry(child, &components, path)?;
                let mut file = lock(&child.item)?;
                let size = file.metadata()?.len();
                file.seek(SeekFrom::Start(0))?;
                read_bounded(&mut *file, size)
            }
            Source::Zip(zip) => zip.read(&components, path),
            Source::FatImage(image) => image.read(&components, path),
            Source::IsoImage(image) => image.read(&components, path),
        }
    }
}

fn single_entry(child: &Child<Mutex<File>>, components: &[&str], path: &str) -> Result<()> {
    find_child(std::slice::from_ref(child), components[0], path)?;
    if components.len() != 1 {
        return Err(MountError::NotDirectory(path.to_owned()));
    }
    Ok(())
}

fn host_entry(root: &Dir, components: &[&str], path: &str) -> Result<(PathBuf, Entry)> {
    let mut relative = PathBuf::new();
    let mut dir = root.try_clone()?;
    for (index, component) in components.iter().enumerate() {
        let mut children = host_children(&dir)?;
        let selected = find_child(&children, component, path)?;
        let entry = children.swap_remove(selected).entry;
        relative.push(&entry.host_name);
        if index + 1 == components.len() {
            return Ok((relative, entry));
        }
        if !entry.is_dir {
            return Err(MountError::NotDirectory(path.to_owned()));
        }
        dir = root.open_dir(&relative)?;
    }
    Err(MountError::IsDirectory(path.to_owned()))
}

fn list_host_children(dir: &Dir) -> Result<Vec<Entry>> {
    Ok(host_children(dir)?
        .into_iter()
        .map(|child| child.entry)
        .collect())
}

fn host_children(dir: &Dir) -> Result<Vec<Child<()>>> {
    let mut children = Vec::new();
    for item in dir.entries()? {
        let item = item?;
        let Ok(name) = item.file_name().into_string() else {
            continue;
        };
        if !valid_component(&name) {
            continue;
        }
        let metadata = match dir.symlink_metadata(&name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let ty = metadata.file_type();
        if ty.is_file() || ty.is_dir() {
            let size = if ty.is_dir() { 0 } else { metadata.len() };
            children.push(Child::new(Entry::new(name, ty.is_dir(), size), ()));
        }
    }
    assign_aliases(&mut children);
    Ok(children)
}

fn read_bounded(reader: impl Read, size: u64) -> Result<Vec<u8>> {
    if size > MAX_FILE_BYTES {
        return Err(MountError::FileTooLarge);
    }
    let mut data = Vec::new();
    reader.take(MAX_FILE_BYTES + 1).read_to_end(&mut data)?;
    if data.len() as u64 > MAX_FILE_BYTES {
        return Err(MountError::FileTooLarge);
    }
    Ok(data)
}

fn lock<T>(value: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    value.lock().map_err(|_| MountError::LockPoisoned)
}
