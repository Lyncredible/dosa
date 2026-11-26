use std::time::SystemTime;

use dosa_mount::{
    DosTimestamp, Entry, FileWriteOptions, Mount, MountError, WritableFile, WriteDisposition,
    attributes,
};
use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

#[path = "file_snapshot.rs"]
mod snapshot;
pub(crate) use snapshot::{FilesSnapshot, SnapshotPipes};

const MAX_OPEN_BYTES: u64 = 256 * 1024 * 1024;
const MAX_HANDLES: u16 = 64;

pub(crate) struct DosFiles {
    pub drive: char,
    pub(crate) denied_write_drive: Option<char>,
    directories: BTreeMap<char, String>,
    handles: BTreeMap<u16, u16>,
    open_files: BTreeMap<u16, Arc<File>>,
    search: VecDeque<(Entry, u8)>,
}

struct File {
    source: FileSource,
    position: Mutex<usize>,
    device_info: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DosDevice {
    Console,
    Null,
}

impl DosDevice {
    /// Recognize a DOS device by its final path component.
    pub fn named(path: &str) -> Option<Self> {
        let name = path.rsplit(['\\', '/', ':']).next()?;
        let stem = name.split('.').next()?;
        if stem.eq_ignore_ascii_case("CON") {
            Some(Self::Console)
        } else if stem.eq_ignore_ascii_case("NUL") {
            Some(Self::Null)
        } else {
            None
        }
    }

    pub(crate) fn ioctl_info(self) -> u16 {
        match self {
            Self::Console => 0x83,
            Self::Null => 0x84,
        }
    }
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) struct DeviceHandle {
    pub(crate) device: DosDevice,
    pub(crate) readable: bool,
    pub(crate) writable: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum HandleTarget {
    Console,
    Null,
    File(u16),
}

enum FileSource {
    Device(DeviceHandle),
    UnavailableDevice,
    Spool {
        bytes: Arc<Mutex<Vec<u8>>>,
        writable: bool,
    },
    ReadOnly {
        contents: Vec<u8>,
        modified: Option<DosTimestamp>,
    },
    Writable {
        file: WritableFile,
        readable: bool,
        writable: bool,
    },
}

impl DosFiles {
    pub(crate) fn new(drive: char, cwd: String) -> Self {
        Self::with_directories(drive, BTreeMap::from([(drive, cwd)]))
    }

    pub(crate) fn with_directories(drive: char, directories: BTreeMap<char, String>) -> Self {
        let mut files = Self {
            drive,
            denied_write_drive: None,
            directories,
            handles: BTreeMap::new(),
            open_files: BTreeMap::new(),
            search: VecDeque::new(),
        };
        for (handle, info) in [(0, 0x81), (1, 0x82), (2, 0x82)] {
            let source = FileSource::Device(DeviceHandle {
                device: DosDevice::Console,
                readable: handle == 0,
                writable: handle != 0,
            });
            files.insert_handle(handle, source, info);
        }
        for handle in [3, 4] {
            files.insert_handle(handle, FileSource::UnavailableDevice, 0x80);
        }
        files
    }

    pub(crate) fn close_all(&mut self) {
        self.handles.clear();
        self.open_files.clear();
        self.search.clear();
    }

    pub(crate) fn directory(&self, drive: char) -> &str {
        self.directories.get(&drive).map_or("", String::as_str)
    }

    fn resolve(&self, path: &str) -> Result<(char, String), u16> {
        let (drive, path) = match path.split_once(':') {
            Some((drive, path))
                if drive.len() == 1 && drive.as_bytes()[0].is_ascii_alphabetic() =>
            {
                (char::from(drive.as_bytes()[0].to_ascii_uppercase()), path)
            }
            Some(_) => return Err(15),
            None => (self.drive, path),
        };
        let mut parts: Vec<_> = if path.starts_with(['\\', '/']) {
            Vec::new()
        } else {
            self.directory(drive)
                .split('\\')
                .filter(|part| !part.is_empty())
                .collect()
        };
        for part in path.split(['\\', '/']) {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                part => parts.push(part),
            }
        }
        Ok((drive, parts.join("\\")))
    }

    fn resolve_mount<'a>(
        &self,
        mounts: &'a BTreeMap<char, Mount>,
        name: &str,
    ) -> Result<(char, &'a Mount, String), u16> {
        let (drive, path) = self.resolve(name)?;
        let mount = mounts.get(&drive).ok_or(15u16)?;
        Ok((drive, mount, path))
    }

    pub(crate) fn chdir(&mut self, mounts: &BTreeMap<char, Mount>, name: &str) -> Result<(), u16> {
        let (drive, mount, path) = self.resolve_mount(mounts, name)?;
        let canonical = mount.canonical_directory(&path).map_err(|_| 3u16)?;
        self.directories.insert(drive, canonical);
        Ok(())
    }

    pub(crate) fn mkdir(&self, mounts: &BTreeMap<char, Mount>, name: &str) -> Result<(), u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        mount.create_dir(&path).map_err(directory_code)
    }

    pub(crate) fn rmdir(&self, mounts: &BTreeMap<char, Mount>, name: &str) -> Result<(), u16> {
        let (drive, mount, path) = self.resolve_mount(mounts, name)?;
        let canonical = mount.canonical_directory(&path).map_err(|_| 3u16)?;
        if canonical.eq_ignore_ascii_case(self.directory(drive)) {
            return Err(16);
        }
        mount.remove_dir(&path).map_err(directory_code)
    }

    fn named_device(
        &self,
        mounts: &BTreeMap<char, Mount>,
        name: &str,
    ) -> Result<Option<DosDevice>, u16> {
        let Some(device) = DosDevice::named(name) else {
            return Ok(None);
        };
        let (drive, path) = self.resolve(name)?;
        let mount = mounts.get(&drive);
        if name.contains(':') && mount.is_none() {
            return Err(15);
        }
        if let Some((parent, _)) = path.rsplit_once('\\') {
            mount
                .ok_or(15u16)?
                .canonical_directory(parent)
                .map_err(directory_code)?;
        }
        Ok(Some(device))
    }

    pub(crate) fn open(
        &mut self,
        mounts: &BTreeMap<char, Mount>,
        name: &str,
        mode: u8,
    ) -> Result<u16, u16> {
        let handle = self.next_handle()?;
        let access = mode & 3;
        if access == 3 {
            return Err(12);
        }
        if let Some(device) = self.named_device(mounts, name)? {
            let source = FileSource::Device(DeviceHandle {
                device,
                readable: access != 1,
                writable: access != 0,
            });
            return Ok(self.insert_handle(handle, source, device.ioctl_info()));
        }
        let (drive, mount, path) = self.resolve_mount(mounts, name)?;
        let source = match access {
            0 if mount.is_writable() => FileSource::Writable {
                file: mount.open_readable_file(&path).map_err(mount_code)?,
                readable: true,
                writable: false,
            },
            0 => {
                let entry = mount.metadata(&path).map_err(mount_code)?;
                FileSource::ReadOnly {
                    contents: self.read_file(mount, &path, entry.size)?,
                    modified: entry.modified,
                }
            }
            1 | 2 => FileSource::Writable {
                file: mount
                    .open_writable_file(&path, WriteDisposition::OpenExisting)
                    .map_err(|error| self.write_open_error(drive, error))?,
                readable: access == 2,
                writable: true,
            },
            _ => return Err(12),
        };
        Ok(self.insert_handle(handle, source, u16::from(drive as u8 - b'A')))
    }

    pub(crate) fn create(
        &mut self,
        mounts: &BTreeMap<char, Mount>,
        name: &str,
        modified: Option<SystemTime>,
    ) -> Result<u16, u16> {
        let handle = self.next_handle()?;
        if let Some(device) = self.named_device(mounts, name)? {
            let source = FileSource::Device(DeviceHandle {
                device,
                readable: true,
                writable: true,
            });
            return Ok(self.insert_handle(handle, source, device.ioctl_info()));
        }
        let (drive, mount, path) = self.resolve_mount(mounts, name)?;
        self.check_file_not_open_for_writing(mount, &path)?;
        let file = mount
            .create_file(&path, modified)
            .map_err(|error| self.write_open_error(drive, error))?;
        let source = FileSource::Writable {
            file,
            readable: true,
            writable: true,
        };
        Ok(self.insert_handle(handle, source, u16::from(drive as u8 - b'A')))
    }

    fn insert_handle(&mut self, handle: u16, source: FileSource, device_info: u16) -> u16 {
        let slot = (0..MAX_HANDLES)
            .find(|slot| !self.open_files.contains_key(slot))
            .expect("an unused handle guarantees room for an open file");
        self.open_files.insert(
            slot,
            Arc::new(File {
                source,
                position: Mutex::new(0),
                device_info,
            }),
        );
        self.handles.insert(handle, slot);
        handle
    }

    fn file(&self, handle: u16) -> Option<&File> {
        self.open_files
            .get(self.handles.get(&handle)?)
            .map(Arc::as_ref)
    }

    pub(crate) fn attach_pipe(&mut self, bytes: Arc<Mutex<Vec<u8>>>, writable: bool) {
        let handle = u16::from(writable);
        let _ = self.close(handle);
        self.insert_handle(handle, FileSource::Spool { bytes, writable }, 0);
    }

    /// Inherit the parent console and file streams without sharing handle assignments.
    pub(crate) fn inherit_standard_handles(&mut self, parent: &Self) {
        let mut inherited = BTreeMap::new();
        for handle in 0..5 {
            let _ = self.close(handle);
            if let Some(&parent_slot) = parent.handles.get(&handle) {
                let slot = *inherited.entry(parent_slot).or_insert_with(|| {
                    let slot = (0..MAX_HANDLES)
                        .find(|slot| !self.open_files.contains_key(slot))
                        .expect("there is room for an inherited standard handle");
                    self.open_files
                        .insert(slot, Arc::clone(&parent.open_files[&parent_slot]));
                    slot
                });
                self.handles.insert(handle, slot);
            }
        }
    }

    fn write_open_error(&mut self, drive: char, error: MountError) -> u16 {
        if matches!(error, MountError::ReadOnly) {
            self.denied_write_drive = Some(drive);
        }
        mount_code(error)
    }

    pub(crate) fn delete(&self, mounts: &BTreeMap<char, Mount>, name: &str) -> Result<(), u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        self.check_file_not_open_for_writing(mount, &path)?;
        mount.remove_file(&path).map_err(mount_code)
    }

    pub(crate) fn rename(
        &self,
        mounts: &BTreeMap<char, Mount>,
        from: &str,
        to: &str,
    ) -> Result<(), u16> {
        let (drive, mount, from) = self.resolve_mount(mounts, from)?;
        let (destination_drive, to) = self.resolve(to)?;
        if drive != destination_drive {
            return Err(17);
        }
        self.check_file_not_open_for_writing(mount, &from)?;
        mount.rename_file(&from, &to).map_err(mount_code)
    }

    fn next_handle(&self) -> Result<u16, u16> {
        (0..MAX_HANDLES)
            .find(|handle| !self.handles.contains_key(handle))
            .ok_or(4)
    }

    fn check_file_not_open_for_writing(&self, mount: &Mount, path: &str) -> Result<(), u16> {
        if self.open_files.values().any(|f| matches!(f.source, FileSource::Writable { .. }))
            && let Ok(target) = mount.open_writable_file(path, WriteDisposition::OpenExisting)
            && self.open_files.values().any(|open| {
                matches!(&open.source, FileSource::Writable { file, .. } if file.same_target(&target))
            })
        {
            return Err(5);
        }
        Ok(())
    }

    fn read_file(&self, mount: &Mount, path: &str, size: u64) -> Result<Vec<u8>, u16> {
        let remaining = MAX_OPEN_BYTES
            - self
                .open_files
                .values()
                .map(|f| match &f.source {
                    FileSource::ReadOnly {
                        contents: bytes, ..
                    } => bytes.len() as u64,
                    FileSource::Device(_)
                    | FileSource::Writable { .. }
                    | FileSource::UnavailableDevice
                    | FileSource::Spool { .. } => 0,
                })
                .sum::<u64>();
        if size > remaining {
            return Err(8);
        }
        let bytes = mount.read(path).map_err(mount_code)?;
        if bytes.len() as u64 > remaining {
            return Err(8);
        }
        Ok(bytes)
    }

    pub(crate) fn attributes(
        &self,
        mounts: &BTreeMap<char, Mount>,
        name: &str,
    ) -> Result<u16, u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        let entry = mount.metadata(&path).map_err(mount_code)?;
        Ok(u16::from(mount.dos_attributes(&entry)))
    }

    pub(crate) fn close(&mut self, handle: u16) -> Result<(), u16> {
        let slot = self.handles.remove(&handle).ok_or(6u16)?;
        if !self.handles.values().any(|&open| open == slot) {
            self.open_files.remove(&slot);
        }
        Ok(())
    }

    pub(crate) fn duplicate(&mut self, handle: u16) -> Result<u16, u16> {
        let slot = *self.handles.get(&handle).ok_or(6u16)?;
        let duplicate = self.next_handle()?;
        self.handles.insert(duplicate, slot);
        Ok(duplicate)
    }

    pub(crate) fn duplicate_to(&mut self, handle: u16, target: u16) -> Result<(), u16> {
        let slot = *self.handles.get(&handle).ok_or(6u16)?;
        if target >= MAX_HANDLES {
            return Err(6);
        }
        if self.handles.get(&target) != Some(&slot) {
            let _ = self.close(target);
            self.handles.insert(target, slot);
        }
        Ok(())
    }

    pub(crate) fn psp_handles(&self) -> [u8; 20] {
        std::array::from_fn(|handle| {
            self.handles
                .get(&(handle as u16))
                .map_or(0xff, |&slot| slot as u8)
        })
    }

    pub(crate) fn ioctl_info(&self, handle: u16) -> Option<u16> {
        Some(self.file(handle)?.device_info)
    }

    pub(crate) fn modified(&self, handle: u16) -> Result<DosTimestamp, u16> {
        let file = self.file(handle).ok_or(6u16)?;
        let modified = match &file.source {
            FileSource::Device(_) | FileSource::Spool { .. } => return Err(1),
            FileSource::UnavailableDevice => return Err(6),
            FileSource::ReadOnly { modified, .. } => *modified,
            FileSource::Writable { file, .. } => file.modified().map_err(mount_code)?,
        };
        Ok(modified.unwrap_or(DosTimestamp::EPOCH))
    }

    pub(crate) fn device(&self, handle: u16) -> Option<DeviceHandle> {
        match self.file(handle)?.source {
            FileSource::Device(channel) => Some(channel),
            _ => None,
        }
    }

    pub(crate) fn is_open(&self, handle: u16) -> bool {
        self.handles.contains_key(&handle)
    }

    pub(crate) fn input_target(&self, handle: u16) -> Result<HandleTarget, u16> {
        match self.file(handle).map(|file| &file.source) {
            Some(FileSource::Device(channel)) if channel.readable => Ok(match channel.device {
                DosDevice::Console => HandleTarget::Console,
                DosDevice::Null => HandleTarget::Null,
            }),
            Some(
                FileSource::ReadOnly { .. }
                | FileSource::Writable { readable: true, .. }
                | FileSource::Spool {
                    writable: false, ..
                },
            ) => Ok(HandleTarget::File(handle)),
            Some(FileSource::UnavailableDevice) | None => Err(6),
            _ => Err(5),
        }
    }

    pub(crate) fn available(&self, handle: u16) -> bool {
        let Some(file) = self.file(handle) else {
            return false;
        };
        let Ok(position) = file.position.lock() else {
            return false;
        };
        let length = match &file.source {
            FileSource::ReadOnly { contents, .. } => contents.len() as u64,
            FileSource::Spool {
                bytes,
                writable: false,
            } => bytes.lock().map_or(0, |b| b.len() as u64),
            FileSource::Writable {
                file,
                readable: true,
                ..
            } => file.length().unwrap_or(0),
            _ => return false,
        };
        (*position as u64) < length
    }

    pub(crate) fn output_target(&self, handle: u16) -> Result<HandleTarget, u16> {
        match self.file(handle).map(|file| &file.source) {
            Some(FileSource::Device(channel)) if channel.writable => Ok(match channel.device {
                DosDevice::Console => HandleTarget::Console,
                DosDevice::Null => HandleTarget::Null,
            }),
            Some(
                FileSource::Writable { writable: true, .. }
                | FileSource::Spool { writable: true, .. },
            ) => Ok(HandleTarget::File(handle)),
            Some(FileSource::UnavailableDevice) | None => Err(6),
            _ => Err(5),
        }
    }

    pub(crate) fn read(&mut self, handle: u16, length: usize) -> Result<Cow<'_, [u8]>, u16> {
        self.read_through(handle, length, None::<fn(u8) -> bool>)
    }

    /// Read at most length bytes, advancing the handle only through the matching byte.
    pub(crate) fn read_through(
        &mut self,
        handle: u16,
        length: usize,
        stop: Option<impl FnMut(u8) -> bool>,
    ) -> Result<Cow<'_, [u8]>, u16> {
        let file = self.file(handle).ok_or(6u16)?;
        let mut position = file.position.lock().map_err(|_| 5u16)?;
        let data = match &file.source {
            FileSource::UnavailableDevice => return Err(6),
            FileSource::Spool {
                bytes,
                writable: false,
            } => {
                let bytes = bytes.lock().map_err(|_| 5u16)?;
                Cow::Owned(read_slice(&bytes, *position, length).to_vec())
            }
            FileSource::Spool { .. } => return Err(5),
            FileSource::ReadOnly {
                contents: bytes, ..
            } => Cow::Borrowed(read_slice(bytes, *position, length)),
            FileSource::Writable {
                file: source,
                readable: true,
                ..
            } => Cow::Owned(
                source
                    .read_at(*position as u64, length)
                    .map_err(mount_code)?,
            ),
            FileSource::Device(_)
            | FileSource::Writable {
                readable: false, ..
            } => return Err(5),
        };
        let used = stop
            .and_then(|predicate| data.iter().copied().position(predicate))
            .map_or(data.len(), |index| index + 1);
        *position += used;
        Ok(match data {
            Cow::Borrowed(bytes) => Cow::Borrowed(&bytes[..used]),
            Cow::Owned(mut bytes) => {
                bytes.truncate(used);
                Cow::Owned(bytes)
            }
        })
    }

    pub(crate) fn write(
        &mut self,
        handle: u16,
        data: &[u8],
        write: FileWriteOptions,
    ) -> Result<(), u16> {
        let file = self.file(handle).ok_or(6u16)?;
        let mut position = file.position.lock().map_err(|_| 5u16)?;
        let end = position
            .checked_add(data.len())
            .filter(|&end| end as u64 <= MAX_OPEN_BYTES)
            .ok_or(112u16)?;
        match &file.source {
            FileSource::Writable {
                file: destination,
                writable: true,
                ..
            } => {
                let mut bytes = destination.read().map_err(mount_code)?;
                update_bytes(&mut bytes, *position, end, data);
                destination
                    .replace_with(&bytes, write)
                    .map_err(mount_code)?;
            }
            FileSource::Spool {
                bytes,
                writable: true,
            } => {
                let mut bytes = bytes.lock().map_err(|_| 5u16)?;
                update_bytes(&mut bytes, *position, end, data);
            }
            _ => return Err(5),
        }
        *position = end;
        Ok(())
    }

    pub(crate) fn seek(&mut self, handle: u16, origin: u8, offset: i32) -> Result<u32, u16> {
        if self.device(handle).is_some() {
            return if origin <= 2 { Ok(0) } else { Err(1) };
        }
        let file = self.file(handle).ok_or(6u16)?;
        if matches!(file.source, FileSource::UnavailableDevice) {
            return Err(6);
        }
        let mut cursor = file.position.lock().map_err(|_| 5u16)?;
        let base = match origin {
            0 => 0,
            1 => *cursor,
            2 => match &file.source {
                FileSource::Device(_) | FileSource::UnavailableDevice => {
                    unreachable!("device handles cannot seek")
                }
                FileSource::ReadOnly {
                    contents: bytes, ..
                } => bytes.len(),
                FileSource::Spool { bytes, .. } => bytes.lock().map_err(|_| 5u16)?.len(),
                FileSource::Writable { file: source, .. } => {
                    source.length().map_err(mount_code)? as usize
                }
            },
            _ => return Err(1),
        };
        let position = u32::try_from(base as i64 + i64::from(offset)).map_err(|_| 1u16)?;
        *cursor = position as usize;
        Ok(position)
    }

    pub(crate) fn find(
        &mut self,
        mounts: &BTreeMap<char, Mount>,
        pattern: &str,
        mask: u16,
    ) -> Result<(Entry, u8), u16> {
        self.search.clear();
        let (_, mount, path) = self.resolve_mount(mounts, pattern)?;
        let (directory, name) = path.rsplit_once('\\').unwrap_or(("", &path));
        self.search = mount
            .list(directory)
            .map_err(|_| 3u16)?
            .into_iter()
            .filter_map(|entry| {
                let special = attributes::DIRECTORY | attributes::HIDDEN | attributes::SYSTEM;
                let flags = mount.dos_attributes(&entry);
                (mask & u16::from(attributes::VOLUME_LABEL) == 0
                    && flags & special & !(mask as u8) == 0
                    && matches_dos_pattern(&entry.name, name))
                .then_some((entry, flags))
            })
            .collect();
        self.next()
    }

    pub(crate) fn next(&mut self) -> Result<(Entry, u8), u16> {
        self.search.pop_front().ok_or(18)
    }
}

fn update_bytes(bytes: &mut Vec<u8>, position: usize, end: usize, data: &[u8]) {
    if data.is_empty() || end > bytes.len() {
        bytes.resize(end, 0);
    }
    if !data.is_empty() {
        bytes[position..end].copy_from_slice(data);
    }
}

fn read_slice(bytes: &[u8], position: usize, length: usize) -> &[u8] {
    let start = position.min(bytes.len());
    let end = start.saturating_add(length).min(bytes.len());
    &bytes[start..end]
}

fn directory_code(error: MountError) -> u16 {
    if error.is_not_found() {
        3
    } else {
        mount_code(error)
    }
}

fn mount_code(error: MountError) -> u16 {
    match error {
        MountError::NotFound(..) => 2,
        MountError::InvalidPath(..) | MountError::NotDirectory(..) => 3,
        MountError::AlreadyExists(..) => 80,
        MountError::FileTooLarge => 8,
        MountError::Io(error) if error.kind() == std::io::ErrorKind::StorageFull => 112,
        MountError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => 2,
        MountError::Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists => 80,
        _ => 5,
    }
}

/// Match the stem and extension of a DOS directory name against a search pattern.
pub fn matches_dos_pattern(name: &str, pattern: &str) -> bool {
    let (stem, extension) = name.split_once('.').unwrap_or((name, ""));
    let (stem_pattern, extension_pattern) = pattern
        .split_once('.')
        .unwrap_or((pattern, if pattern.contains('*') { "*" } else { "" }));
    glob(stem.as_bytes(), stem_pattern.as_bytes())
        && glob(extension.as_bytes(), extension_pattern.as_bytes())
}

fn glob(mut name: &[u8], mut pattern: &[u8]) -> bool {
    let mut retry = None;
    loop {
        match pattern.split_first() {
            Some((&b'*', rest)) => {
                retry = Some((name, rest));
                pattern = rest;
            }
            Some((&character, rest))
                if name
                    .first()
                    .is_some_and(|b| character == b'?' || character.eq_ignore_ascii_case(b)) =>
            {
                name = &name[1..];
                pattern = rest;
            }
            None if name.is_empty() => return true,
            _ => {
                let Some(([_first, remaining @ ..], rest)) = retry else {
                    return false;
                };
                name = remaining;
                pattern = rest;
                retry = Some((remaining, rest));
            }
        }
    }
}
