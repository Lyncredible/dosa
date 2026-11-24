use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use fatfs::{Dir, DirEntry, FileSystem, FsOptions};

use super::{Entry, MAX_FILE_BYTES, MountError, Result, lock, read_bounded, valid_component};

const READ_BUDGET: u64 = MAX_FILE_BYTES * 2;
const READ_OPERATIONS: usize = 2_000_000;
const MAX_DIRECTORY_ENTRIES: usize = 65_536;

#[derive(Debug)]
pub(super) struct FatImage(Mutex<File>);

impl FatImage {
    pub fn open(mut file: File) -> Result<Self> {
        let length = file.metadata()?.len();
        let mut header = [0; 512];
        file.read_exact(&mut header)?;
        let bytes_per_sector = u16::from_le_bytes([header[11], header[12]]);
        let sectors16 = u16::from_le_bytes([header[19], header[20]]);
        let sectors = if sectors16 == 0 {
            u32::from_le_bytes(header[32..36].try_into().unwrap())
        } else {
            u32::from(sectors16)
        };
        if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096)
            || sectors == 0
            || u64::from(bytes_per_sector) * u64::from(sectors) > length
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid or truncated FAT volume geometry",
            )
            .into());
        }
        let image = Self(Mutex::new(file));
        image.with_volume(|_| Ok(()))?;
        Ok(image)
    }

    fn with_volume<T>(
        &self,
        operation: impl FnOnce(Dir<'_, ImageReader<'_>>) -> Result<T>,
    ) -> Result<T> {
        let mut file = lock(&self.0)?;
        if !file.metadata()?.is_file() {
            return Err(MountError::UnsupportedSource);
        }
        file.rewind()?;
        let volume = FileSystem::new(
            ImageReader {
                file: &mut file,
                remaining: READ_BUDGET,
                operations: READ_OPERATIONS,
            },
            FsOptions::new().update_accessed_date(false),
        )?;
        operation(volume.root_dir())
    }

    pub fn list(&self, components: &[&str], path: &str) -> Result<Vec<Entry>> {
        self.with_volume(|mut dir| {
            for component in components {
                let entry = find(&dir, component, path)?;
                if !entry.is_dir() {
                    return Err(MountError::NotDirectory(path.into()));
                }
                dir = entry.to_dir();
            }
            let mut entries = Vec::new();
            for (index, entry) in dir.iter().enumerate() {
                if index >= MAX_DIRECTORY_ENTRIES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "FAT directory exceeds the entry limit",
                    )
                    .into());
                }
                let entry = entry?;
                if let Some(info) = info(&entry) {
                    entries.push(info);
                }
            }
            Ok(entries)
        })
    }

    pub fn metadata(&self, components: &[&str], path: &str) -> Result<Entry> {
        self.with_volume(|dir| {
            info(&open_entry(dir, components, path)?)
                .ok_or_else(|| MountError::NotFound(path.into()))
        })
    }

    pub fn read(&self, components: &[&str], path: &str) -> Result<Vec<u8>> {
        self.with_volume(|dir| {
            let entry = open_entry(dir, components, path)?;
            if entry.is_dir() {
                return Err(MountError::IsDirectory(path.into()));
            }
            read_bounded(entry.to_file(), entry.len())
        })
    }
}

fn open_entry<'a, 'b>(
    mut dir: Dir<'a, ImageReader<'b>>,
    components: &[&str],
    path: &str,
) -> Result<DirEntry<'a, ImageReader<'b>>> {
    let (last, parents) = components
        .split_last()
        .ok_or_else(|| MountError::IsDirectory(path.into()))?;
    for component in parents {
        let entry = find(&dir, component, path)?;
        if !entry.is_dir() {
            return Err(MountError::NotDirectory(path.into()));
        }
        dir = entry.to_dir();
    }
    find(&dir, last, path)
}

fn find<'a, 'b>(
    dir: &Dir<'a, ImageReader<'b>>,
    name: &str,
    path: &str,
) -> Result<DirEntry<'a, ImageReader<'b>>> {
    for entry in dir.iter().take(MAX_DIRECTORY_ENTRIES) {
        let entry = entry?;
        if let Some(info) = info(&entry)
            && (info.name.eq_ignore_ascii_case(name) || info.host_name.eq_ignore_ascii_case(name))
        {
            return Ok(entry);
        }
    }
    Err(MountError::NotFound(path.into()))
}

fn info(entry: &DirEntry<'_, ImageReader<'_>>) -> Option<Entry> {
    let name = entry.short_file_name();
    if name == "." || name == ".." || !valid_component(&name) {
        return None;
    }
    let long = entry.file_name();
    let host_name = if valid_component(&long) {
        long
    } else {
        name.clone()
    };
    Some(Entry {
        name,
        host_name,
        is_dir: entry.is_dir(),
        size: entry.len(),
    })
}

struct ImageReader<'a> {
    file: &'a mut File,
    remaining: u64,
    operations: usize,
}

impl Read for ImageReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 || self.operations == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "FAT image read budget exhausted",
            ));
        }
        self.operations -= 1;
        let limit = bytes.len().min(self.remaining as usize);
        let read = self.file.read(&mut bytes[..limit])?;
        self.remaining -= read as u64;
        Ok(read)
    }
}

impl Seek for ImageReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}

impl Write for ImageReader<'_> {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::ErrorKind::PermissionDenied.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
