use dosa_mount::{Entry, Mount, MountError, WritableFile, WriteDisposition};
use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};

const MAX_OPEN_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) struct DosFiles {
    pub drive: char,
    directories: BTreeMap<char, String>,
    handles: BTreeMap<u16, File>,
    search: VecDeque<(Entry, u8)>,
}

struct File {
    source: FileSource,
    position: usize,
}

enum FileSource {
    ReadOnly(Vec<u8>),
    Writable {
        file: WritableFile,
        readable: bool,
        writable: bool,
    },
}

impl DosFiles {
    pub(crate) fn new(drive: char, cwd: String) -> Self {
        Self {
            drive,
            directories: BTreeMap::from([(drive, cwd)]),
            handles: BTreeMap::new(),
            search: VecDeque::new(),
        }
    }

    pub(crate) fn close_all(&mut self) {
        self.handles.clear();
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
        let canonical = canonical_directory(mount, &path)?;
        self.directories.insert(drive, canonical);
        Ok(())
    }

    pub(crate) fn mkdir(&self, mounts: &BTreeMap<char, Mount>, name: &str) -> Result<(), u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        mount.create_dir(&path).map_err(directory_code)
    }

    pub(crate) fn rmdir(&self, mounts: &BTreeMap<char, Mount>, name: &str) -> Result<(), u16> {
        let (drive, mount, path) = self.resolve_mount(mounts, name)?;
        let canonical = canonical_directory(mount, &path)?;
        if canonical.eq_ignore_ascii_case(self.directory(drive)) {
            return Err(16);
        }
        mount.remove_dir(&path).map_err(directory_code)
    }

    pub(crate) fn open(
        &mut self,
        mounts: &BTreeMap<char, Mount>,
        name: &str,
        mode: u8,
    ) -> Result<u16, u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        let handle = self.next_handle()?;
        let source = match mode & 3 {
            0 if mount.is_writable() => FileSource::Writable {
                file: mount.open_readable_file(&path).map_err(mount_code)?,
                readable: true,
                writable: false,
            },
            0 => FileSource::ReadOnly(self.read_file(mount, &path)?),
            1 | 2 => FileSource::Writable {
                file: mount
                    .open_writable_file(&path, WriteDisposition::OpenExisting)
                    .map_err(mount_code)?,
                readable: mode & 3 == 2,
                writable: true,
            },
            _ => return Err(12),
        };
        self.handles.insert(
            handle,
            File {
                source,
                position: 0,
            },
        );
        Ok(handle)
    }

    pub(crate) fn create(
        &mut self,
        mounts: &BTreeMap<char, Mount>,
        name: &str,
    ) -> Result<u16, u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        let handle = self.next_handle()?;
        self.check_file_not_open_for_writing(mount, &path)?;
        let file = mount
            .open_writable_file(&path, WriteDisposition::CreateOrTruncate)
            .map_err(mount_code)?;
        let source = FileSource::Writable {
            file,
            readable: true,
            writable: true,
        };
        self.handles.insert(
            handle,
            File {
                source,
                position: 0,
            },
        );
        Ok(handle)
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
        (5..64)
            .find(|handle| !self.handles.contains_key(handle))
            .ok_or(4)
    }

    fn check_file_not_open_for_writing(&self, mount: &Mount, path: &str) -> Result<(), u16> {
        if self.handles.values().any(|f| matches!(f.source, FileSource::Writable { .. }))
            && let Ok(target) = mount.open_writable_file(path, WriteDisposition::OpenExisting)
            && self.handles.values().any(|open| {
                matches!(&open.source, FileSource::Writable { file, .. } if file.same_target(&target))
            })
        {
            return Err(5);
        }
        Ok(())
    }

    fn read_file(&self, mount: &Mount, path: &str) -> Result<Vec<u8>, u16> {
        let remaining = MAX_OPEN_BYTES
            - self
                .handles
                .values()
                .map(|f| match &f.source {
                    FileSource::ReadOnly(bytes) => bytes.len() as u64,
                    FileSource::Writable { .. } => 0,
                })
                .sum::<u64>();
        if mount.metadata(path).map_err(mount_code)?.size > remaining {
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
        Ok(u16::from(attributes(&entry, !mount.is_writable())))
    }

    pub(crate) fn close(&mut self, handle: u16) -> Result<(), u16> {
        self.handles.remove(&handle).map(|_| ()).ok_or(6)
    }

    pub(crate) fn is_open(&self, handle: u16) -> bool {
        self.handles.contains_key(&handle)
    }

    pub(crate) fn check_write_handle(&self, handle: u16) -> Result<(), u16> {
        match self.handles.get(&handle).map(|file| &file.source) {
            Some(FileSource::Writable { writable: true, .. }) => Ok(()),
            Some(FileSource::Writable {
                writable: false, ..
            }) => Err(5),
            Some(FileSource::ReadOnly(_)) => Err(5),
            None => Err(6),
        }
    }

    pub(crate) fn read(&mut self, handle: u16, length: usize) -> Result<Cow<'_, [u8]>, u16> {
        let file = self.handles.get_mut(&handle).ok_or(6u16)?;
        let data = match &file.source {
            FileSource::ReadOnly(bytes) => Cow::Borrowed(read_slice(bytes, file.position, length)),
            FileSource::Writable {
                file: source,
                readable: true,
                ..
            } => Cow::Owned(
                source
                    .read_at(file.position as u64, length)
                    .map_err(mount_code)?,
            ),
            FileSource::Writable {
                readable: false, ..
            } => return Err(5),
        };
        file.position += data.len();
        Ok(data)
    }

    pub(crate) fn write(&mut self, handle: u16, data: &[u8]) -> Result<(), u16> {
        let file = self.handles.get_mut(&handle).ok_or(6u16)?;
        let FileSource::Writable {
            file: destination,
            writable: true,
            ..
        } = &file.source
        else {
            return Err(5);
        };
        let end = file
            .position
            .checked_add(data.len())
            .filter(|&end| end as u64 <= MAX_OPEN_BYTES)
            .ok_or(112u16)?;
        let mut bytes = destination.read().map_err(mount_code)?;
        if data.is_empty() {
            bytes.resize(end, 0);
        } else {
            if end > bytes.len() {
                bytes.resize(end, 0);
            }
            bytes[file.position..end].copy_from_slice(data);
        }
        destination.replace(&bytes).map_err(mount_code)?;
        file.position = end;
        Ok(())
    }

    pub(crate) fn seek(&mut self, handle: u16, origin: u8, offset: i32) -> Result<u32, u16> {
        let file = self.handles.get_mut(&handle).ok_or(6u16)?;
        let base = match origin {
            0 => 0,
            1 => file.position,
            2 => match &file.source {
                FileSource::ReadOnly(bytes) => bytes.len(),
                FileSource::Writable { file: source, .. } => {
                    source.length().map_err(mount_code)? as usize
                }
            },
            _ => return Err(1),
        };
        let position = u32::try_from(base as i64 + i64::from(offset)).map_err(|_| 1u16)?;
        file.position = position as usize;
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
            .filter(|entry| {
                (!entry.is_dir || mask & 0x10 != 0)
                    && mask & 0x08 == 0
                    && matches(&entry.name, name)
            })
            .map(|entry| {
                let attributes = attributes(&entry, !mount.is_writable());
                (entry, attributes)
            })
            .collect();
        self.next()
    }

    pub(crate) fn next(&mut self) -> Result<(Entry, u8), u16> {
        self.search.pop_front().ok_or(18)
    }
}

fn attributes(entry: &Entry, read_only: bool) -> u8 {
    if entry.is_dir {
        0x10
    } else {
        0x20 | u8::from(read_only)
    }
}

fn canonical_directory(mount: &Mount, path: &str) -> Result<String, u16> {
    let mut canonical = String::new();
    for part in path.split('\\').filter(|part| !part.is_empty()) {
        let candidate = if canonical.is_empty() {
            part.to_owned()
        } else {
            format!("{canonical}\\{part}")
        };
        let entry = mount.metadata(&candidate).map_err(|_| 3u16)?;
        if !entry.is_dir {
            return Err(3);
        }
        if !canonical.is_empty() {
            canonical.push('\\');
        }
        canonical.push_str(&entry.name);
    }
    Ok(canonical)
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

fn matches(name: &str, pattern: &str) -> bool {
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
