use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use fatfs::{Dir, DirEntry, FileSystem, FsOptions};

use super::{Entry, MAX_FILE_BYTES, MountError, Result, lock, read_bounded, valid_component};

const READ_BUDGET: u64 = MAX_FILE_BYTES * 2;
const READ_OPERATIONS: usize = 2_000_000;
const MAX_DIRECTORY_ENTRIES: usize = 65_536;
const MBR_SECTOR_BYTES: u64 = 512;
const MAX_LOGICAL_PARTITIONS: u16 = 128;

#[derive(Debug)]
pub(super) struct FatImage {
    file: Mutex<File>,
    region: Region,
}

#[derive(Clone, Copy, Debug)]
struct Region {
    start: u64,
    bytes: u64,
}

impl FatImage {
    pub fn open(mut file: File, partition: Option<u16>) -> Result<Self> {
        let length = file.metadata()?.len();
        let mut header = [0; 512];
        file.read_exact(&mut header)?;

        // A volume image can contain executable boot code in the area where an
        // MBR stores entries. Try a valid filesystem at sector zero first.
        let raw = volume_bytes(&header, length).and_then(|bytes| {
            let region = Region { start: 0, bytes };
            open_volume(&mut file, region)?.unmount()?;
            Ok(region)
        });
        let region = match (raw, partition) {
            (Ok(region), None) => region,
            (raw, partition) => {
                let Some(mut region) = mbr_fat_partition(&mut file, &header, length, partition)?
                else {
                    let error = match partition {
                        Some(number) => io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("FAT partition {number} was not found"),
                        ),
                        None => raw.expect_err("raw volume already handled"),
                    };
                    return Err(error.into());
                };
                file.seek(SeekFrom::Start(region.start))?;
                file.read_exact(&mut header)?;
                region.bytes = volume_bytes(&header, region.bytes)?;
                open_volume(&mut file, region)?.unmount()?;
                region
            }
        };
        Ok(Self {
            file: Mutex::new(file),
            region,
        })
    }

    fn with_volume<T>(
        &self,
        operation: impl FnOnce(Dir<'_, ImageReader<'_>>) -> Result<T>,
    ) -> Result<T> {
        let mut file = lock(&self.file)?;
        if !file.metadata()?.is_file() {
            return Err(MountError::UnsupportedSource);
        }
        let volume = open_volume(&mut file, self.region)?;
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

fn volume_bytes(header: &[u8; 512], capacity: u64) -> io::Result<u64> {
    let bytes_per_sector = u16::from_le_bytes([header[11], header[12]]);
    let sectors16 = u16::from_le_bytes([header[19], header[20]]);
    let sectors = if sectors16 == 0 {
        u32::from_le_bytes(header[32..36].try_into().unwrap())
    } else {
        u32::from(sectors16)
    };
    let bytes = u64::from(bytes_per_sector) * u64::from(sectors);
    if !matches!(bytes_per_sector, 512 | 1024 | 2048 | 4096) || bytes == 0 || bytes > capacity {
        return Err(invalid_data("invalid or truncated FAT volume geometry"));
    }
    Ok(bytes)
}

fn mbr_fat_partition(
    file: &mut File,
    header: &[u8; 512],
    length: u64,
    selected: Option<u16>,
) -> io::Result<Option<Region>> {
    if header[510..] != [0x55, 0xaa] {
        return Ok(None);
    }
    let mut regions = Vec::<Region>::new();
    let mut fat = None;
    let mut extended = None;
    for (index, entry) in header[446..510].as_chunks::<16>().0.iter().enumerate() {
        let Some((kind, region)) =
            partition_entry(entry, 0, length, "invalid or truncated MBR partition")?
        else {
            continue;
        };
        if regions.iter().any(|other| {
            region.start < other.start + other.bytes && other.start < region.start + region.bytes
        }) {
            return Err(invalid_data("overlapping MBR partitions"));
        }
        regions.push(region);
        if is_fat(kind) && selected.is_none_or(|number| number == (index + 1) as u16) {
            fat.get_or_insert(region);
        }
        if is_extended(kind) && extended.replace(region).is_some() {
            return Err(invalid_data("multiple MBR extended partitions"));
        }
    }
    match (fat, extended) {
        (None, Some(container)) if selected.is_none_or(|number| number >= 5) => {
            logical_fat_partition(file, container, selected)
        }
        _ => Ok(fat),
    }
}

fn logical_fat_partition(
    file: &mut File,
    container: Region,
    selected: Option<u16>,
) -> io::Result<Option<Region>> {
    let end = container.start + container.bytes;
    let mut current = container.start;
    let mut header = [0; 512];
    for number in 5..5 + MAX_LOGICAL_PARTITIONS {
        file.seek(SeekFrom::Start(current))?;
        file.read_exact(&mut header)?;
        if header[510..] != [0x55, 0xaa] {
            return Err(invalid_data("invalid extended boot record signature"));
        }
        let entries = header[446..510].as_chunks::<16>().0;
        let data = partition_entry(
            &entries[0],
            current,
            end,
            "logical partition exceeds its extended container",
        )?;
        let mut minimum_next = current + MBR_SECTOR_BYTES;
        if let Some((kind, region)) = data {
            if is_extended(kind) {
                return Err(invalid_data("invalid logical partition type"));
            }
            if is_fat(kind) && selected.is_none_or(|wanted| wanted == number) {
                return Ok(Some(region));
            }
            minimum_next = region.start + region.bytes;
        }
        // Data starts relative to its own EBR. The chain link is relative
        // to the start of the enclosing extended partition.
        let link = partition_entry(
            &entries[1],
            container.start,
            end,
            "invalid extended boot record link",
        )?;
        let Some((kind, next)) = link else {
            return Ok(None);
        };
        if !is_extended(kind) || next.start < minimum_next {
            return Err(invalid_data(
                "backward or overlapping extended boot record link",
            ));
        }
        current = next.start;
    }
    Err(invalid_data("too many logical partitions"))
}

fn partition_entry(
    entry: &[u8; 16],
    base: u64,
    end: u64,
    message: &'static str,
) -> io::Result<Option<(u8, Region)>> {
    let kind = entry[4];
    if kind == 0 {
        return Ok(None);
    }
    let offset = u64::from(u32::from_le_bytes(entry[8..12].try_into().unwrap())) * MBR_SECTOR_BYTES;
    let bytes = u64::from(u32::from_le_bytes(entry[12..16].try_into().unwrap())) * MBR_SECTOR_BYTES;
    let start = base + offset;
    if !matches!(entry[0], 0 | 0x80) || offset == 0 || bytes == 0 || start + bytes > end {
        return Err(invalid_data(message));
    }
    Ok(Some((kind, Region { start, bytes })))
}

fn is_fat(kind: u8) -> bool {
    matches!(kind, 0x01 | 0x04 | 0x06 | 0x0b | 0x0c | 0x0e)
}

fn is_extended(kind: u8) -> bool {
    matches!(kind, 0x05 | 0x0f | 0x85)
}

fn open_volume(file: &mut File, region: Region) -> io::Result<FileSystem<ImageReader<'_>>> {
    file.seek(SeekFrom::Start(region.start))?;
    FileSystem::new(
        ImageReader {
            file,
            region,
            position: 0,
            remaining: READ_BUDGET,
            operations: READ_OPERATIONS,
        },
        FsOptions::new().update_accessed_date(false),
    )
}

fn invalid_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
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
    region: Region,
    position: u64,
    remaining: u64,
    operations: usize,
}

impl Read for ImageReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() || self.position == self.region.bytes {
            return Ok(0);
        }
        if self.remaining == 0 || self.operations == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "FAT image read budget exhausted",
            ));
        }
        self.operations -= 1;
        let available = self.remaining.min(self.region.bytes - self.position);
        let limit = (bytes.len() as u64).min(available) as usize;
        let read = self.file.read(&mut bytes[..limit])?;
        self.remaining -= read as u64;
        self.position += read as u64;
        Ok(read)
    }
}

impl Seek for ImageReader<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let offset = match position {
            SeekFrom::Start(offset) => Some(offset),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
            SeekFrom::End(delta) => self.region.bytes.checked_add_signed(delta),
        }
        .filter(|&offset| offset <= self.region.bytes)
        .ok_or_else(|| invalid_data("FAT seek leaves the mounted volume"))?;
        self.file
            .seek(SeekFrom::Start(self.region.start + offset))?;
        self.position = offset;
        Ok(offset)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_reader_cannot_seek_or_read_beyond_the_volume() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"xxFATtail").unwrap();
        file.seek(SeekFrom::Start(2)).unwrap();
        let mut reader = ImageReader {
            file: &mut file,
            region: Region { start: 2, bytes: 3 },
            position: 0,
            remaining: READ_BUDGET,
            operations: READ_OPERATIONS,
        };
        assert!(reader.seek(SeekFrom::Current(-1)).is_err());
        assert_eq!(reader.seek(SeekFrom::End(-1)).unwrap(), 2);
        let mut tail = Vec::new();
        reader.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, b"T");
        assert!(reader.seek(SeekFrom::End(1)).is_err());
        assert_eq!(
            reader
                .rewind()
                .and_then(|()| reader.stream_position())
                .unwrap(),
            0
        );
        let mut contents = Vec::new();
        reader.read_to_end(&mut contents).unwrap();
        assert_eq!(contents, b"FAT");
    }
}
