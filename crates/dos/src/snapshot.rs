//! Versioned snapshots of the DOS runtime. Mounted files remain host-owned.

use super::*;
use crate::{audio::AudioSnapshot, files::FilesSnapshot, video::VideoSnapshot, xms::XmsSnapshot};
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 8] = b"DOSAGST\0";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 18;
/// Maximum size in bytes of a serialized guest save state.
pub const MAX_GUEST_SNAPSHOT_BYTES: usize = 300 * 1024 * 1024;
const MAX_ELAPSED: Duration = Duration::from_secs(366 * 86_400);
const MAX_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
const MAX_MEDIA_ENTRIES: usize = 32_768;

/// An error saving a guest or preparing a replacement guest.
#[derive(Debug, thiserror::Error)]
pub enum GuestSnapshotError {
    #[error("invalid or damaged guest save state")]
    Invalid,
    #[error("unsupported guest save state version {0}")]
    Version(u16),
    #[error("save state unavailable because {0}")]
    Unavailable(&'static str),
    #[error("the mounted media differ from the saved session")]
    MediaChanged,
    #[error("could not inspect mounted media: {0}")]
    Media(#[from] MountError),
    #[error("could not restore the guest CPU: {0}")]
    Cpu(#[from] dosa_cpu::SnapshotError),
}

#[derive(Serialize, Deserialize)]
struct State<'a> {
    profile: CpuProfile,
    elapsed: Duration,
    paused: bool,
    session_clock: Option<Duration>,
    media: BTreeMap<char, u32>,
    state: RunState,
    pending: Vec<PendingCall>,
    files: FilesSnapshot<'a>,
    environment: BTreeMap<String, String>,
    video: VideoSnapshot,
    keys: VecDeque<(u8, u8)>,
    keyboard: Keyboard,
    fast_a20: bool,
    xms: XmsSnapshot,
    gameport: gameport::GamePort,
    mouse: Mouse,
    pic: Pic,
    pit: Pit,
    audio: AudioSnapshot,
    modifiers: u8,
    locks: u8,
    output: Vec<u8>,
    verify_writes: bool,
    extended_break: bool,
    dta: (u16, u16),
    blocks: BTreeMap<u16, u16>,
    line: Vec<u8>,
}

impl Machine {
    /// Save an active DOS process and its devices at an instruction boundary.
    pub fn save_guest(&self) -> Result<Vec<u8>, GuestSnapshotError> {
        use GuestSnapshotError::{Invalid, Unavailable};
        if !self.state.is_active() || self.boot_disk.is_some() {
            return Err(Unavailable("an active DOS process is required"));
        }
        let elapsed = self.elapsed();
        if elapsed > MAX_ELAPSED {
            return Err(Unavailable("the guest has been running too long"));
        }
        let state = State {
            profile: self.cpu_profile,
            elapsed,
            paused: self.paused_since.is_some(),
            session_clock: self.session_clock.snapshot(),
            media: fingerprints(&self.mounts)?,
            state: self.state,
            pending: self.pending.clone(),
            files: self.files.snapshot().map_err(Unavailable)?,
            environment: self
                .environment
                .variables()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
            video: self.video.snapshot(),
            keys: self.keys.clone(),
            keyboard: self.keyboard.clone(),
            fast_a20: self.fast_a20,
            xms: self.xms.snapshot(),
            gameport: self.gameport.clone(),
            mouse: self.mouse.clone(),
            pic: self.pic.clone(),
            pit: self.pit.clone(),
            audio: self.audio.snapshot().map_err(Unavailable)?,
            modifiers: self.modifiers,
            locks: self.locks,
            output: self.output.clone(),
            verify_writes: self.verify_writes,
            extended_break: self.extended_break,
            dta: self.dta,
            blocks: self.blocks.clone(),
            line: self.line.clone(),
        };
        let mut metadata = postcard::to_allocvec(&state).map_err(|_| Invalid)?;
        let cpu = self.cpu.save_snapshot()?;
        let metadata_len = u32::try_from(metadata.len()).map_err(|_| Invalid)?;
        if HEADER_BYTES + metadata.len() + cpu.len() > MAX_GUEST_SNAPSHOT_BYTES {
            return Err(Unavailable("the guest save state is too large"));
        }
        let mut checksum = crc32fast::Hasher::new();
        checksum.update(&metadata);
        checksum.update(&cpu);
        let mut bytes = Vec::with_capacity(HEADER_BYTES + metadata.len() + cpu.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&metadata_len.to_le_bytes());
        bytes.extend_from_slice(&checksum.finalize().to_le_bytes());
        bytes.append(&mut metadata);
        bytes.extend_from_slice(&cpu);
        Ok(bytes)
    }

    /// Load a saved DOS process once the entire state and mounted media validate.
    pub fn load_guest(&mut self, bytes: &[u8]) -> Result<(), GuestSnapshotError> {
        use GuestSnapshotError::{Invalid, MediaChanged, Unavailable};
        if self.audio.mpu.configured() {
            return Err(Unavailable("MIDI synthesis is configured"));
        }
        if !(HEADER_BYTES..=MAX_GUEST_SNAPSHOT_BYTES).contains(&bytes.len()) || &bytes[..8] != MAGIC
        {
            return Err(Invalid);
        }
        let version = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if version != VERSION {
            return Err(GuestSnapshotError::Version(version));
        }
        let metadata_len = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
        let checksum = u32::from_le_bytes(bytes[14..18].try_into().unwrap());
        let payload = &bytes[HEADER_BYTES..];
        let (metadata, cpu_bytes) = payload.split_at_checked(metadata_len).ok_or(Invalid)?;
        if crc32fast::hash(payload) != checksum {
            return Err(Invalid);
        }
        let (state, trailing): (State<'static>, _) =
            postcard::take_from_bytes(metadata).map_err(|_| Invalid)?;
        if !trailing.is_empty()
            || !state.state.is_active()
            || state.elapsed > MAX_ELAPSED
            || state.keys.len() > MAX_KEYS
            || state.line.len() > 255
            || !state.keyboard.valid_snapshot()
            || !state.mouse.valid_snapshot()
            || !state.pit.valid_snapshot(state.elapsed)
        {
            return Err(Invalid);
        }
        if fingerprints(&self.mounts)? != state.media {
            return Err(MediaChanged);
        }
        let mut environment = Environment::default();
        environment.set("COMSPEC", "").map_err(|_| Invalid)?;
        environment.set("BLASTER", "").map_err(|_| Invalid)?;
        for (key, value) in state.environment {
            environment.set(&key, &value).map_err(|_| Invalid)?;
        }
        let session_clock =
            clock::SessionClock::from_snapshot(state.session_clock).ok_or(Invalid)?;
        let files = DosFiles::from_snapshot(state.files).ok_or(Invalid)?;
        let audio = audio::Audio::from_snapshot(state.audio, state.elapsed).ok_or(Invalid)?;
        let mut video = Adapter::from_snapshot(state.video).ok_or(Invalid)?;
        let mut cpu = Cpu::with_profile(state.profile);
        if !video.bind_snapshot_aperture(&mut cpu) {
            return Err(Invalid);
        }
        cpu.restore_snapshot(cpu_bytes)?;
        let xms = xms::Xms::from_snapshot(state.xms, cpu.memory.len()).ok_or(Invalid)?;
        video.refresh(&cpu);
        let now = Instant::now();
        let clock = now.checked_sub(state.elapsed).ok_or(Invalid)?;
        *self = Self {
            cpu,
            cpu_profile: state.profile,
            video,
            mounts: std::mem::take(&mut self.mounts),
            boot_disk: None,
            environment,
            files,
            state: state.state,
            pending: state.pending,
            keys: state.keys,
            keyboard: state.keyboard,
            fast_a20: state.fast_a20,
            xms,
            gameport: state.gameport,
            mouse: state.mouse,
            pic: state.pic,
            pit: state.pit,
            audio,
            modifiers: state.modifiers,
            locks: state.locks,
            output: state.output,
            clock,
            session_clock,
            verify_writes: state.verify_writes,
            extended_break: state.extended_break,
            paused_since: state.paused.then_some(now),
            dta: state.dta,
            blocks: state.blocks,
            line: state.line,
        };
        Ok(())
    }
}

fn fingerprints(mounts: &BTreeMap<char, Mount>) -> Result<BTreeMap<char, u32>, GuestSnapshotError> {
    let mut total = 0;
    let mut entry_count = 0;
    let mut media = BTreeMap::new();
    for (&drive, mount) in mounts {
        if mount.iso_sector_count().is_some() {
            return Err(GuestSnapshotError::Unavailable(
                "an optical image is mounted",
            ));
        }
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(mount.description().as_bytes());
        let mut directories = vec![String::new()];
        while let Some(directory) = directories.pop() {
            let mut entries = mount.list(&directory)?;
            entries.sort_unstable_by(|left, right| left.name.cmp(&right.name));
            for entry in entries {
                entry_count += 1;
                if entry_count > MAX_MEDIA_ENTRIES {
                    return Err(GuestSnapshotError::Unavailable(
                        "mounted media have too many entries",
                    ));
                }
                let path = if directory.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{directory}\\{}", entry.name)
                };
                let record = postcard::to_allocvec(&(&path, &entry))
                    .map_err(|_| GuestSnapshotError::Invalid)?;
                hasher.update(&record);
                if entry.is_dir {
                    directories.push(path);
                } else {
                    if entry.size > MAX_MEDIA_BYTES - total {
                        return Err(GuestSnapshotError::Unavailable(
                            "mounted media are too large",
                        ));
                    }
                    let bytes = mount.read(&path)?;
                    if bytes.len() as u64 != entry.size {
                        return Err(GuestSnapshotError::MediaChanged);
                    }
                    total += entry.size;
                    hasher.update(&bytes);
                }
            }
        }
        media.insert(drive, hasher.finalize());
    }
    Ok(media)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Directory;

    #[test]
    fn restores_a_waiting_guest_on_a_new_machine_with_matching_media() {
        let directory = Directory::new("save-guest");
        directory.write(
            "STATE.COM",
            &[
                0xb4, 0, 0xcd, 0x16, // INT 16h, wait for keyboard input.
                0x88, 0xc2, 0xb4, 2, 0xcd, 0x21, // Print the key.
                0xb8, 0x2a, 0x4c, 0xcd, 0x21, // Exit with errorlevel 42.
            ],
        );
        let mut original = directory.machine();
        original.launch('C', "STATE.COM", "").unwrap();
        assert_eq!(original.run(100).unwrap(), RunState::WaitingForKey);
        original.set_paused(true);
        let bytes = original.save_guest().unwrap();

        let mut restored = directory.machine();
        restored.load_guest(&bytes).unwrap();
        let paused = restored.paused_guest().unwrap();
        assert_eq!(paused.state, RunState::WaitingForKey);
        assert_eq!(
            paused.registers(),
            original.paused_guest().unwrap().registers()
        );

        restored.set_paused(false);
        restored.key(b'Z', 0x2c);
        assert_eq!(restored.run(100).unwrap(), RunState::Exited(42));
        assert_eq!(restored.take_output(), b"Z");
    }

    #[test]
    fn rejects_corrupt_state_and_changed_media_without_replacing_the_guest() {
        let directory = Directory::new("verify-guest-media");
        directory.write("LOOP.COM", &[0xeb, 0xfe]);
        directory.write("DATA.TXT", b"before");
        let mut machine = directory.machine();
        machine.launch('C', "LOOP.COM", "").unwrap();
        machine.set_paused(true);
        let mut bytes = machine.save_guest().unwrap();
        let registers = machine.paused_guest().unwrap().registers();

        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            machine.load_guest(&bytes),
            Err(GuestSnapshotError::Invalid)
        ));
        bytes[last] ^= 1;
        assert_eq!(machine.paused_guest().unwrap().registers(), registers);

        directory.write("DATA.TXT", b"change");
        assert!(matches!(
            machine.load_guest(&bytes),
            Err(GuestSnapshotError::MediaChanged)
        ));
        assert_eq!(machine.paused_guest().unwrap().registers(), registers);
    }
}
