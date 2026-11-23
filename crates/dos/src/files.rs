use dosa_mount::{Entry, Mount, MountError};
use std::collections::{BTreeMap, VecDeque};

const MAX_OPEN_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) struct DosFiles {
    pub drive: char,
    directories: BTreeMap<char, String>,
    handles: BTreeMap<u16, File>,
    search: VecDeque<Entry>,
}

struct File {
    bytes: Vec<u8>,
    position: usize,
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
        self.directories.insert(drive, canonical);
        Ok(())
    }

    pub(crate) fn open(&mut self, mounts: &BTreeMap<char, Mount>, name: &str) -> Result<u16, u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        let handle = (5..64)
            .find(|handle| !self.handles.contains_key(handle))
            .ok_or(4u16)?;
        let remaining = MAX_OPEN_BYTES
            - self
                .handles
                .values()
                .map(|f| f.bytes.len() as u64)
                .sum::<u64>();
        if mount.metadata(&path).map_err(mount_code)?.size > remaining {
            return Err(8);
        }
        let bytes = mount.read(&path).map_err(mount_code)?;
        if bytes.len() as u64 > remaining {
            return Err(8);
        }
        self.handles.insert(handle, File { bytes, position: 0 });
        Ok(handle)
    }

    pub(crate) fn attributes(
        &self,
        mounts: &BTreeMap<char, Mount>,
        name: &str,
    ) -> Result<u16, u16> {
        let (_, mount, path) = self.resolve_mount(mounts, name)?;
        let entry = mount.metadata(&path).map_err(mount_code)?;
        Ok(u16::from(attributes(&entry)))
    }

    pub(crate) fn close(&mut self, handle: u16) -> Result<(), u16> {
        self.handles.remove(&handle).map(|_| ()).ok_or(6)
    }

    pub(crate) fn is_open(&self, handle: u16) -> bool {
        self.handles.contains_key(&handle)
    }

    pub(crate) fn read(&mut self, handle: u16, length: usize) -> Result<&[u8], u16> {
        let file = self.handles.get_mut(&handle).ok_or(6u16)?;
        let start = file.position.min(file.bytes.len());
        let end = (start + length).min(file.bytes.len());
        file.position += end - start;
        Ok(&file.bytes[start..end])
    }

    pub(crate) fn seek(&mut self, handle: u16, origin: u8, offset: i32) -> Result<u32, u16> {
        let file = self.handles.get_mut(&handle).ok_or(6u16)?;
        let base = match origin {
            0 => 0,
            1 => file.position,
            2 => file.bytes.len(),
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
    ) -> Result<Entry, u16> {
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
            .collect();
        self.next()
    }

    pub(crate) fn next(&mut self) -> Result<Entry, u16> {
        self.search.pop_front().ok_or(18)
    }
}

pub(crate) fn attributes(entry: &Entry) -> u8 {
    if entry.is_dir { 0x10 } else { 0x21 }
}

fn mount_code(error: MountError) -> u16 {
    match error {
        MountError::NotFound(..) => 2,
        MountError::InvalidPath(..) | MountError::NotDirectory(..) => 3,
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
