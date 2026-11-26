use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::*;
use crate::ShellPipe;

pub(crate) const MAX_SNAPSHOT_PIPES: usize = 1024;

#[derive(Default)]
pub(crate) struct SnapshotPipes {
    ids: HashMap<*const Mutex<Vec<u8>>, u16>,
    pipes: Vec<ShellPipe>,
}

impl SnapshotPipes {
    pub(crate) fn register(&mut self, bytes: &Arc<Mutex<Vec<u8>>>) -> Result<u16, &'static str> {
        let key = Arc::as_ptr(bytes);
        if let Some(&id) = self.ids.get(&key) {
            return Ok(id);
        }
        if self.pipes.len() == MAX_SNAPSHOT_PIPES {
            return Err("too many shell pipe buffers");
        }
        let id = self.pipes.len() as u16;
        self.ids.insert(key, id);
        self.pipes.push(ShellPipe(Arc::clone(bytes)));
        Ok(id)
    }

    pub(crate) fn buffers(&self) -> Result<Vec<Vec<u8>>, &'static str> {
        let mut total = 0;
        self.pipes
            .iter()
            .map(|pipe| {
                let bytes = pipe.0.lock().map_err(|_| "shell pipe lock failed")?;
                total += bytes.len();
                if total > 32 * 1024 * 1024 {
                    return Err("buffered pipe data exceed the save limit");
                }
                Ok(bytes.clone())
            })
            .collect()
    }

    pub(crate) fn restore(buffers: Vec<Vec<u8>>) -> Option<Vec<ShellPipe>> {
        if buffers.len() > MAX_SNAPSHOT_PIPES
            || buffers.iter().map(Vec::len).sum::<usize>() > 32 * 1024 * 1024
        {
            return None;
        }
        Some(
            buffers
                .into_iter()
                .map(|bytes| ShellPipe(Arc::new(Mutex::new(bytes))))
                .collect(),
        )
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FilesSnapshot<'a> {
    drive: char,
    denied_write_drive: Option<char>,
    directories: BTreeMap<char, String>,
    handles: BTreeMap<u16, u16>,
    open_files: BTreeMap<u16, OpenFile<'a>>,
    search: VecDeque<(Entry, u8)>,
}

#[derive(Serialize, Deserialize)]
struct OpenFile<'a> {
    source: Source<'a>,
    position: usize,
    device_info: u16,
}

#[derive(Serialize, Deserialize)]
enum Source<'a> {
    Device(DeviceHandle),
    UnavailableDevice,
    ReadOnly {
        contents: Cow<'a, [u8]>,
        modified: Option<DosTimestamp>,
    },
    WritableMount {
        target: String,
        readable: bool,
        writable: bool,
    },
    Shared(u16),
    Pipe {
        id: u16,
        writable: bool,
    },
}

impl DosFiles {
    pub(crate) fn snapshot_shell_io<'a>(
        &'a self,
        mounts: &BTreeMap<char, Mount>,
        streams: &[&'a crate::ShellIo],
        mut pipes: Option<&mut SnapshotPipes>,
    ) -> Result<Vec<u8>, &'static str> {
        if streams.len() > 16 {
            return Err("too many shell file tables");
        }
        let mut files = HashMap::new();
        for file in self.open_files.values() {
            let next = files.len() as u16;
            if files.insert(Arc::as_ptr(file), next).is_some() {
                return Err("duplicate guest file slot");
            }
        }
        let states = streams
            .iter()
            .map(|stream| {
                stream
                    .files
                    .capture(mounts, Some(&mut files), pipes.as_deref_mut())
            })
            .collect::<Result<Vec<_>, _>>()?;
        postcard::to_allocvec(&states).map_err(|_| "shell file tables could not be encoded")
    }

    pub(crate) fn restore_shell_io(
        &self,
        mounts: &BTreeMap<char, Mount>,
        bytes: &[u8],
        count: usize,
        pipes: &[ShellPipe],
    ) -> Option<Vec<crate::ShellIo>> {
        if count > 16 {
            return None;
        }
        let (states, trailing): (Vec<FilesSnapshot<'static>>, _) =
            postcard::take_from_bytes(bytes).ok()?;
        if states.len() != count || !trailing.is_empty() {
            return None;
        }
        let mut shared: Vec<_> = self.open_files.values().cloned().collect();
        states
            .into_iter()
            .map(|state| {
                Self::restore(state, mounts, Some(&mut shared), pipes)
                    .map(|files| crate::ShellIo { files })
            })
            .collect()
    }

    pub(crate) fn snapshot(
        &self,
        mounts: &BTreeMap<char, Mount>,
        pipes: Option<&mut SnapshotPipes>,
    ) -> Result<FilesSnapshot<'_>, &'static str> {
        self.capture(mounts, None, pipes)
    }

    fn capture(
        &self,
        mounts: &BTreeMap<char, Mount>,
        mut shared: Option<&mut HashMap<*const File, u16>>,
        mut pipes: Option<&mut SnapshotPipes>,
    ) -> Result<FilesSnapshot<'_>, &'static str> {
        let mut open_files = BTreeMap::new();
        for (&slot, file) in &self.open_files {
            if let Some(shared) = shared.as_deref_mut() {
                let next = shared.len() as u16;
                match shared.entry(Arc::as_ptr(file)) {
                    std::collections::hash_map::Entry::Occupied(id) => {
                        open_files.insert(
                            slot,
                            OpenFile {
                                source: Source::Shared(*id.get()),
                                position: 0,
                                device_info: 0,
                            },
                        );
                        continue;
                    }
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(next);
                    }
                }
            }
            let source = match &file.source {
                FileSource::Device(channel) => Source::Device(*channel),
                FileSource::UnavailableDevice => Source::UnavailableDevice,
                FileSource::ReadOnly { contents, modified } => Source::ReadOnly {
                    contents: Cow::Borrowed(contents),
                    modified: *modified,
                },
                FileSource::Spool { bytes, writable } => Source::Pipe {
                    id: pipes
                        .as_deref_mut()
                        .ok_or("a shell pipeline is active")?
                        .register(bytes)?,
                    writable: *writable,
                },
                FileSource::Writable {
                    file: live,
                    readable,
                    writable,
                } => {
                    let target = live.target_path();
                    let mount = file_mount(mounts, file.device_info)
                        .ok_or("an open file's drive is unavailable")?;
                    let reopened = mount
                        .reopen_live_file(&target, *writable)
                        .map_err(|_| "an open file could not be reopened")?;
                    if !live.same_target(&reopened) {
                        return Err("an open file's drive has changed");
                    }
                    Source::WritableMount {
                        target,
                        readable: *readable,
                        writable: *writable,
                    }
                }
            };
            let position = *file.position.lock().map_err(|_| "open file lock failed")?;
            open_files.insert(
                slot,
                OpenFile {
                    source,
                    position,
                    device_info: file.device_info,
                },
            );
        }
        Ok(FilesSnapshot {
            drive: self.drive,
            denied_write_drive: self.denied_write_drive,
            directories: self.directories.clone(),
            handles: self.handles.clone(),
            open_files,
            search: self.search.clone(),
        })
    }

    pub(crate) fn from_snapshot(
        state: FilesSnapshot<'_>,
        mounts: &BTreeMap<char, Mount>,
        pipes: &[ShellPipe],
    ) -> Option<Self> {
        Self::restore(state, mounts, None, pipes)
    }

    fn restore(
        state: FilesSnapshot<'_>,
        mounts: &BTreeMap<char, Mount>,
        mut shared: Option<&mut Vec<Arc<File>>>,
        pipes: &[ShellPipe],
    ) -> Option<Self> {
        if !state.drive.is_ascii_uppercase()
            || state
                .denied_write_drive
                .is_some_and(|drive| !drive.is_ascii_uppercase())
            || !state.directories.keys().all(char::is_ascii_uppercase)
            || !state
                .handles
                .iter()
                .all(|(&handle, slot)| handle < MAX_HANDLES && state.open_files.contains_key(slot))
            || !state.open_files.keys().all(|&slot| {
                slot < MAX_HANDLES && state.handles.values().any(|target| *target == slot)
            })
        {
            return None;
        }
        let mut open_files = BTreeMap::new();
        let mut bytes = 0;
        for (slot, file) in state.open_files {
            if u32::try_from(file.position).is_err() {
                return None;
            }
            let source = match file.source {
                Source::Device(channel) => FileSource::Device(channel),
                Source::UnavailableDevice => FileSource::UnavailableDevice,
                Source::ReadOnly { contents, modified } => {
                    bytes += contents.len() as u64;
                    if bytes > MAX_OPEN_BYTES {
                        return None;
                    }
                    FileSource::ReadOnly {
                        contents: contents.into_owned(),
                        modified,
                    }
                }
                Source::WritableMount {
                    target,
                    readable,
                    writable,
                } => {
                    if !readable && !writable {
                        return None;
                    }
                    let mount = file_mount(mounts, file.device_info)?;
                    let file = mount.reopen_live_file(&target, writable).ok()?;
                    FileSource::Writable {
                        file,
                        readable,
                        writable,
                    }
                }
                Source::Shared(id) => {
                    if file.position != 0 || file.device_info != 0 {
                        return None;
                    }
                    let existing = shared.as_ref()?.get(usize::from(id))?;
                    open_files.insert(slot, Arc::clone(existing));
                    continue;
                }
                Source::Pipe { id, writable } => FileSource::Spool {
                    bytes: Arc::clone(&pipes.get(usize::from(id))?.0),
                    writable,
                },
            };
            let file = Arc::new(File {
                source,
                position: Mutex::new(file.position),
                device_info: file.device_info,
            });
            if let Some(shared) = shared.as_deref_mut() {
                shared.push(Arc::clone(&file));
            }
            open_files.insert(slot, file);
        }

        Some(Self {
            drive: state.drive,
            denied_write_drive: state.denied_write_drive,
            directories: state.directories,
            handles: state.handles,
            open_files,
            search: state.search,
        })
    }
}

fn file_mount(mounts: &BTreeMap<char, Mount>, device_info: u16) -> Option<&Mount> {
    if device_info >= 26 {
        return None;
    }
    mounts.get(&char::from(b'A' + device_info as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_duplicate_handles_at_one_shared_file_position() {
        let mut files = DosFiles::new('C', "\\GUEST".into());
        files.insert_handle(
            5,
            FileSource::ReadOnly {
                contents: b"abcdef".to_vec(),
                modified: Some(DosTimestamp::EPOCH),
            },
            2,
        );
        let duplicate = files.duplicate(5).unwrap();
        assert_eq!(files.seek(5, 0, 2), Ok(2));

        let mounts = BTreeMap::new();
        let bytes = postcard::to_allocvec(&files.snapshot(&mounts, None).unwrap()).unwrap();
        let snapshot: FilesSnapshot<'static> = postcard::from_bytes(&bytes).unwrap();
        let mut restored = DosFiles::from_snapshot(snapshot, &mounts, &[]).unwrap();
        assert_eq!(restored.directory('C'), "\\GUEST");
        assert_eq!(restored.read(duplicate, 2).unwrap().as_ref(), b"cd");
        assert_eq!(restored.read(5, 2).unwrap().as_ref(), b"ef");
        assert_eq!(restored.modified(duplicate), Ok(DosTimestamp::EPOCH));
        assert!(restored.device(0).unwrap().readable);

        let mut snapshot: FilesSnapshot<'static> = postcard::from_bytes(&bytes).unwrap();
        snapshot.handles.insert(63, 63);
        assert!(DosFiles::from_snapshot(snapshot, &mounts, &[]).is_none());
    }
}
