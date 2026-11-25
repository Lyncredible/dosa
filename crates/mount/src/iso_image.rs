//! Read-only ISO 9660 primary volume and directory records (ECMA-119).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Mutex;

use super::{Entry, MountError, Result, lock, read_bounded};
use crate::names::{Child, assign_aliases, find_child, valid_component};

pub const ISO_SECTOR_BYTES: usize = 2048;
const SECTOR: usize = ISO_SECTOR_BYTES;
const DIRECTORY_BUDGET: u64 = 16 * 1024 * 1024;
const MAX_DIRECTORY_ENTRIES: usize = 65_536;

#[derive(Clone, Copy, Debug)]
struct Extent {
    offset: u64,
    size: u64,
}

#[derive(Debug)]
pub(super) struct IsoImage {
    file: Mutex<File>,
    root: Extent,
    volume_size: u64,
    volume_label: String,
}

impl IsoImage {
    pub fn volume_label(&self) -> &str {
        &self.volume_label
    }

    pub fn sector_count(&self) -> u32 {
        (self.volume_size / SECTOR as u64) as u32
    }

    pub fn read_sectors(&self, first: u32, bytes: &mut [u8]) -> Result<()> {
        if bytes.is_empty() || !bytes.len().is_multiple_of(SECTOR) {
            return Err(invalid("ISO 9660 sector buffer must contain whole sectors"));
        }
        let offset = u64::from(first) * SECTOR as u64;
        if offset > self.volume_size || bytes.len() as u64 > self.volume_size - offset {
            return Err(invalid("ISO 9660 sector read lies outside the volume"));
        }
        let mut file = lock(&self.file)?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(bytes)?;
        Ok(())
    }

    pub fn open(mut file: File) -> Result<Self> {
        let image_size = file.metadata()?.len();
        let mut primary = None;
        let mut descriptor = [0; SECTOR];
        file.seek(SeekFrom::Start(16 * SECTOR as u64))?;
        for _ in 0..128 {
            file.read_exact(&mut descriptor)?;
            if &descriptor[1..=5] != b"CD001"
                || matches!(descriptor[0], 1 | 255) && descriptor[6] != 1
            {
                return Err(invalid("invalid ISO 9660 volume descriptor"));
            }
            match descriptor[0] {
                1 if primary.is_none() => {
                    let size = u64::from(both_u32(&descriptor[80..88])?) * SECTOR as u64;
                    if both_u16(&descriptor[128..132])? != SECTOR as u16
                        || both_u16(&descriptor[120..124])? != 1
                        || both_u16(&descriptor[124..128])? != 1
                        || descriptor[881] != 1
                        || size > image_size
                    {
                        return Err(invalid("unsupported or truncated ISO 9660 volume geometry"));
                    }
                    let (root, directory, _) = record(&descriptor[156..190], size)?;
                    if !directory {
                        return Err(invalid("invalid ISO 9660 root directory"));
                    }
                    let label = String::from_utf8_lossy(&descriptor[40..72]).into_owned();
                    primary = Some((root, size, label));
                }
                255 => {
                    let (root, volume_size, volume_label) =
                        primary.ok_or_else(|| invalid("ISO 9660 primary volume is missing"))?;
                    if file.stream_position()? > volume_size {
                        return Err(invalid("ISO 9660 descriptor lies outside the volume"));
                    }
                    let image = Self {
                        file: Mutex::new(file),
                        root,
                        volume_size,
                        volume_label,
                    };
                    image.list(&[], "")?;
                    return Ok(image);
                }
                _ => (),
            }
        }
        Err(invalid("ISO 9660 descriptor terminator is missing"))
    }

    pub fn list(&self, components: &[&str], path: &str) -> Result<Vec<Entry>> {
        let mut file = lock(&self.file)?;
        let mut budget = DIRECTORY_BUDGET;
        let item = self.resolve(&mut file, components, path, &mut budget)?;
        if !item.entry.is_dir {
            return Err(MountError::NotDirectory(path.into()));
        }
        Ok(self
            .directory(&mut file, item.item, &mut budget)?
            .into_iter()
            .map(|child| child.entry)
            .collect())
    }

    pub fn metadata(&self, components: &[&str], path: &str) -> Result<Entry> {
        let mut budget = DIRECTORY_BUDGET;
        Ok(self
            .resolve(&mut *lock(&self.file)?, components, path, &mut budget)?
            .entry)
    }

    pub fn read(&self, components: &[&str], path: &str) -> Result<Vec<u8>> {
        let mut file = lock(&self.file)?;
        let mut budget = DIRECTORY_BUDGET;
        let entry = self.resolve(&mut file, components, path, &mut budget)?;
        if entry.entry.is_dir {
            return Err(MountError::IsDirectory(path.into()));
        }
        file.seek(SeekFrom::Start(entry.item.offset))?;
        let bytes = read_bounded((&mut *file).take(entry.item.size), entry.item.size)?;
        if bytes.len() as u64 != entry.item.size {
            return Err(invalid("truncated ISO 9660 file data"));
        }
        Ok(bytes)
    }

    fn resolve(
        &self,
        file: &mut File,
        components: &[&str],
        path: &str,
        budget: &mut u64,
    ) -> Result<Child<Extent>> {
        let mut entry = Child::new(Entry::root(), self.root);
        for component in components {
            if !entry.entry.is_dir {
                return Err(MountError::NotDirectory(path.into()));
            }
            let mut children = self.directory(file, entry.item, budget)?;
            let index = find_child(&children, component, path)?;
            entry = children.swap_remove(index);
        }
        Ok(entry)
    }

    fn directory(
        &self,
        file: &mut File,
        extent: Extent,
        budget: &mut u64,
    ) -> Result<Vec<Child<Extent>>> {
        if extent.size == 0 || !extent.size.is_multiple_of(SECTOR as u64) {
            return Err(invalid("invalid ISO 9660 directory length"));
        }
        *budget = budget
            .checked_sub(extent.size)
            .ok_or_else(|| invalid("ISO 9660 directory read budget exceeded"))?;
        file.seek(SeekFrom::Start(extent.offset))?;
        let mut entries = BTreeMap::<String, (u16, Child<Extent>)>::new();
        let mut sector = [0; SECTOR];
        for _ in 0..extent.size / SECTOR as u64 {
            file.read_exact(&mut sector)?;
            let mut remaining = &sector[..];
            while let Some(length) = remaining.first().copied().filter(|&length| length != 0) {
                let (data, rest) = remaining
                    .split_at_checked(usize::from(length))
                    .ok_or_else(|| invalid("ISO 9660 directory record crosses a sector"))?;
                remaining = rest;
                let (extent, directory, identifier) = record(data, self.volume_size)?;
                if matches!(identifier, [0] | [1]) || data[25] & 4 != 0 {
                    continue;
                }
                let identifier = std::str::from_utf8(identifier)
                    .map_err(|_| invalid("invalid ISO 9660 identifier"))?;
                let (name, version) = if directory {
                    (identifier, 0)
                } else if let Some((name, version)) = identifier.rsplit_once(';') {
                    (
                        name,
                        version
                            .parse::<u16>()
                            .map_err(|_| invalid("invalid ISO 9660 file version"))?,
                    )
                } else {
                    (identifier, 0)
                };
                let name = name.strip_suffix('.').unwrap_or(name);
                if !valid_component(name) {
                    return Err(invalid("invalid ISO 9660 pathname"));
                }
                let key = name.to_ascii_uppercase();
                if entries
                    .get(&key)
                    .is_none_or(|(existing, _)| version > *existing)
                {
                    let mut entry = Entry::new(name.into(), directory, extent.size);
                    entry.modified = super::DosTimestamp::from_fields(
                        1900 + u16::from(data[18]),
                        data[19],
                        data[20],
                        data[21],
                        data[22],
                        data[23],
                    );
                    if data[25] & 1 != 0 {
                        entry.attributes |= super::attributes::HIDDEN;
                    }
                    entries.insert(key, (version, Child::new(entry, extent)));
                }
                if entries.len() > MAX_DIRECTORY_ENTRIES {
                    return Err(invalid("ISO 9660 directory entry limit exceeded"));
                }
            }
        }
        let mut children: Vec<_> = entries.into_values().map(|(_, child)| child).collect();
        assign_aliases(&mut children);
        Ok(children)
    }
}

fn record(bytes: &[u8], volume_size: u64) -> Result<(Extent, bool, &[u8])> {
    if bytes.len() < 34 || usize::from(bytes[0]) != bytes.len() {
        return Err(invalid("invalid ISO 9660 directory record length"));
    }
    let identifier = bytes
        .get(33..33 + usize::from(bytes[32]))
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid("invalid ISO 9660 identifier length"))?;
    if bytes[25] & 0x80 != 0 || bytes[26] != 0 || bytes[27] != 0 || both_u16(&bytes[28..32])? != 1 {
        return Err(invalid("unsupported ISO 9660 extent layout"));
    }
    let block = u64::from(both_u32(&bytes[2..10])?) + u64::from(bytes[1]);
    let offset = block * SECTOR as u64;
    let size = u64::from(both_u32(&bytes[10..18])?);
    if offset + size > volume_size {
        return Err(invalid("ISO 9660 extent lies outside the volume"));
    }
    Ok((Extent { offset, size }, bytes[25] & 2 != 0, identifier))
}

fn both_u16(bytes: &[u8]) -> Result<u16> {
    let first = u16::from_le_bytes(bytes[..2].try_into().unwrap());
    if first != u16::from_be_bytes(bytes[2..].try_into().unwrap()) {
        return Err(invalid("ISO 9660 number disagrees between byte orders"));
    }
    Ok(first)
}

fn both_u32(bytes: &[u8]) -> Result<u32> {
    let first = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    if first != u32::from_be_bytes(bytes[4..].try_into().unwrap()) {
        return Err(invalid("ISO 9660 number disagrees between byte orders"));
    }
    Ok(first)
}

fn invalid(message: &'static str) -> MountError {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}
