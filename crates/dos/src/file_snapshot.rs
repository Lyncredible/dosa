use serde::{Deserialize, Serialize};

use super::*;

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
}

impl DosFiles {
    pub(crate) fn snapshot(
        &self,
        mounts: &BTreeMap<char, Mount>,
    ) -> Result<FilesSnapshot<'_>, &'static str> {
        let mut open_files = BTreeMap::new();
        for (&slot, file) in &self.open_files {
            let source = match &file.source {
                FileSource::Device(channel) => Source::Device(*channel),
                FileSource::UnavailableDevice => Source::UnavailableDevice,
                FileSource::ReadOnly { contents, modified } => Source::ReadOnly {
                    contents: Cow::Borrowed(contents),
                    modified: *modified,
                },
                FileSource::Spool { .. } => return Err("a shell pipeline is active"),
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
            };
            open_files.insert(
                slot,
                Arc::new(File {
                    source,
                    position: Mutex::new(file.position),
                    device_info: file.device_info,
                }),
            );
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
        let bytes = postcard::to_allocvec(&files.snapshot(&mounts).unwrap()).unwrap();
        let snapshot: FilesSnapshot<'static> = postcard::from_bytes(&bytes).unwrap();
        let mut restored = DosFiles::from_snapshot(snapshot, &mounts).unwrap();
        assert_eq!(restored.directory('C'), "\\GUEST");
        assert_eq!(restored.read(duplicate, 2).unwrap().as_ref(), b"cd");
        assert_eq!(restored.read(5, 2).unwrap().as_ref(), b"ef");
        assert_eq!(restored.modified(duplicate), Ok(DosTimestamp::EPOCH));
        assert!(restored.device(0).unwrap().readable);

        let mut snapshot: FilesSnapshot<'static> = postcard::from_bytes(&bytes).unwrap();
        snapshot.handles.insert(63, 63);
        assert!(DosFiles::from_snapshot(snapshot, &mounts).is_none());
    }
}
