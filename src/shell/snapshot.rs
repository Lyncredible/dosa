use std::collections::BTreeMap;
use std::error::Error;

use dosa_dos::MAX_GUEST_SNAPSHOT_BYTES;
use serde::{Deserialize, Serialize};

use super::{Batch, QuickLaunch, Shell};

const MAGIC: &[u8; 8] = b"DOSASHL1";
const HEADER_BYTES: usize = 16;
const MAX_SHELL_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_DESKTOP_SNAPSHOT_BYTES: usize =
    HEADER_BYTES + MAX_SHELL_BYTES + MAX_GUEST_SNAPSHOT_BYTES;
const INVALID: &str = "invalid or damaged shell save state";

#[derive(Serialize, Deserialize)]
struct State<Dirs, BatchState, Scope> {
    drive: char,
    directories: Dirs,
    batch: BatchState,
    errorlevel: u8,
    quick_launch: Scope,
}

type SavedState = State<BTreeMap<char, String>, Option<Batch>, Option<QuickLaunch>>;

impl Shell {
    pub(crate) fn supports_save_state(&self) -> bool {
        self.pipelines.is_empty()
            && self.pending_input.is_none()
            && self.pending_redirects.is_none()
            && self.batch.as_ref().is_none_or(Batch::supports_snapshot)
    }

    pub(crate) fn save_guest(&self) -> Result<Vec<u8>, Box<dyn Error>> {
        if !self.supports_save_state() {
            return Err("save state unavailable with shell pipes or batch redirection".into());
        }
        let guest = self.machine.save_guest()?;
        if self.batch.is_none() && self.quick_launch.is_none() {
            return Ok(guest);
        }
        let state = State {
            drive: self.drive,
            directories: &self.directories,
            batch: &self.batch,
            errorlevel: self.errorlevel,
            quick_launch: &self.quick_launch,
        };
        let metadata = postcard::to_allocvec(&state)?;
        if metadata.len() > MAX_SHELL_BYTES {
            return Err("shell save state is too large".into());
        }
        let mut crc = crc32fast::Hasher::new();
        crc.update(&metadata);
        crc.update(&guest);
        let mut bytes = Vec::with_capacity(HEADER_BYTES + metadata.len() + guest.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(metadata.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&crc.finalize().to_le_bytes());
        bytes.extend_from_slice(&metadata);
        bytes.extend_from_slice(&guest);
        Ok(bytes)
    }

    pub(crate) fn load_guest(&mut self, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
        let (state, guest) = if bytes.starts_with(MAGIC) {
            let (state, guest) = self.prepare_snapshot(bytes)?;
            (Some(state), guest)
        } else {
            (None, bytes)
        };
        self.machine.load_guest(guest)?;
        self.pending_input = None;
        self.pending_redirects = None;
        self.pipelines.clear();
        if let Some(state) = state {
            self.drive = state.drive;
            self.directories = state.directories;
            self.batch = state.batch;
            self.errorlevel = state.errorlevel;
            self.quick_launch = state.quick_launch;
        } else {
            self.batch = None;
        }
        Ok(())
    }

    fn prepare_snapshot<'a>(
        &self,
        bytes: &'a [u8],
    ) -> Result<(SavedState, &'a [u8]), Box<dyn Error>> {
        if !(HEADER_BYTES..=MAX_DESKTOP_SNAPSHOT_BYTES).contains(&bytes.len()) {
            return Err(INVALID.into());
        }
        let length = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let checksum = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        let payload = &bytes[HEADER_BYTES..];
        if length > MAX_SHELL_BYTES || crc32fast::hash(payload) != checksum {
            return Err(INVALID.into());
        }
        let (metadata, guest) = payload.split_at_checked(length).ok_or(INVALID)?;
        let (mut state, trailing): (SavedState, _) =
            postcard::take_from_bytes(metadata).map_err(|_| INVALID)?;
        if !trailing.is_empty()
            || !state.drive.is_ascii_uppercase()
            || !state.directories.keys().all(char::is_ascii_uppercase)
            || state.quick_launch.as_ref().is_some_and(|scope| {
                !scope.drive.is_ascii_uppercase()
                    || self.machine.drive(scope.drive).is_none()
                    || !scope.previous_drive.is_ascii_uppercase()
                    || scope.previous_directories.contains_key(&scope.drive)
                    || !scope
                        .previous_directories
                        .keys()
                        .all(char::is_ascii_uppercase)
            })
            || state
                .batch
                .as_mut()
                .is_some_and(|batch| !batch.prepare_snapshot())
        {
            return Err(INVALID.into());
        }
        Ok((state, guest))
    }
}
