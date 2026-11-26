//! Versioned snapshots of the DOS runtime. Mounted files remain host-owned.

use super::*;
use crate::{
    adlib::SynthReplay,
    audio::AudioSnapshot,
    boot_disk::{BootDisk, DiskSnapshot},
    files::{FilesSnapshot, SnapshotPipes},
    mpu401::{MidiReplay, MidiSnapshot, Mpu401},
    video::VideoSnapshot,
    xms::XmsSnapshot,
};
use serde::{Deserialize, Serialize};

const MAGIC: &[u8; 8] = b"DOSAGST\0";
const VERSION_LEGACY: u16 = 1;
const VERSION_ADLIB_REPLAY: u16 = 2;
const VERSION_BOOT_MEDIA: u16 = 3;
const VERSION_MIDI: u16 = 4;
const HEADER_BYTES: usize = 18;
/// Maximum size in bytes of a serialized guest save state.
pub const MAX_GUEST_SNAPSHOT_BYTES: usize = 300 * 1024 * 1024;
const MAX_ELAPSED: Duration = Duration::from_secs(366 * 86_400);
const MAX_MEDIA_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_MEDIA_ENTRIES: usize = 32_768;

/// The resumed file tables and pending pipe buffers for the shell.
pub struct RestoredPipelineIo {
    pub streams: Vec<ShellIo>,
    pub pipes: Vec<Option<ShellPipe>>,
}

#[derive(Serialize, Deserialize)]
struct PipelineIo {
    streams: Vec<u8>,
    buffers: Vec<Vec<u8>>,
    pending: Vec<Option<u16>>,
}

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
    #[error("could not inspect the boot disk: {0}")]
    BootMedia(#[from] std::io::Error),
    #[error("the configured MIDI SoundFont differs from the saved session")]
    MidiSoundFontChanged,
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
    /// Save the guest and attached shell file tables, preserving shared open files.
    pub fn save_guest_with_shell_io(
        &self,
        streams: &[&ShellIo],
    ) -> Result<(Vec<u8>, Vec<u8>), GuestSnapshotError> {
        let guest = self.save_guest()?;
        let streams = self
            .files
            .snapshot_shell_io(&self.mounts, streams, None)
            .map_err(GuestSnapshotError::Unavailable)?;
        Ok((guest, streams))
    }

    /// Save an active guest and its devices at an instruction boundary.
    pub fn save_guest(&self) -> Result<Vec<u8>, GuestSnapshotError> {
        self.save_guest_snapshot(None)
    }

    /// Save the guest with shell handles and all buffers reachable from its pipeline.
    pub fn save_guest_with_pipeline_io(
        &self,
        streams: &[&ShellIo],
        pending: &[Option<&ShellPipe>],
    ) -> Result<(Vec<u8>, Vec<u8>), GuestSnapshotError> {
        use GuestSnapshotError::{Invalid, Unavailable};
        let mut pipes = SnapshotPipes::default();
        let pending = pending
            .iter()
            .map(|pipe| pipe.map(|pipe| pipes.register(&pipe.0)).transpose())
            .collect::<Result<_, _>>()
            .map_err(Unavailable)?;
        let guest = self.save_guest_snapshot(Some(&mut pipes))?;
        let streams = self
            .files
            .snapshot_shell_io(&self.mounts, streams, Some(&mut pipes))
            .map_err(Unavailable)?;
        let buffers = pipes.buffers().map_err(Unavailable)?;
        let shell = postcard::to_allocvec(&PipelineIo {
            streams,
            buffers,
            pending,
        })
        .map_err(|_| Invalid)?;
        Ok((guest, shell))
    }

    fn save_guest_snapshot(
        &self,
        pipes: Option<&mut SnapshotPipes>,
    ) -> Result<Vec<u8>, GuestSnapshotError> {
        use GuestSnapshotError::{Invalid, Unavailable};
        if !self.state.is_active() {
            return Err(Unavailable("an active guest is required"));
        }
        let elapsed = self.elapsed();
        if elapsed > MAX_ELAPSED {
            return Err(Unavailable("the guest has been running too long"));
        }
        let replay = self.audio.adlib_replay().map_err(Unavailable)?;
        let midi = self.audio.mpu.snapshot().map_err(Unavailable)?;
        let state = State {
            profile: self.cpu_profile,
            elapsed,
            paused: self.paused_since.is_some(),
            session_clock: self.session_clock.snapshot(),
            media: fingerprints(
                &self.mounts,
                self.boot_disk.as_ref().map_or(0, BootDisk::media_bytes),
            )?,
            state: self.state,
            pending: self.pending.clone(),
            files: self
                .files
                .snapshot(&self.mounts, pipes)
                .map_err(Unavailable)?,
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
            audio: self.audio.snapshot(),
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
        let boot = self
            .boot_disk
            .as_ref()
            .map(BootDisk::snapshot)
            .transpose()?;
        let version = match (boot, replay, midi) {
            (boot, replay, Some(midi)) => {
                metadata =
                    postcard::to_extend(&(boot, replay, midi), metadata).map_err(|_| Invalid)?;
                VERSION_MIDI
            }
            (Some(disk), replay, None) => {
                metadata = postcard::to_extend(&(disk, replay), metadata).map_err(|_| Invalid)?;
                VERSION_BOOT_MEDIA
            }
            (None, Some(replay), None) => {
                metadata = postcard::to_extend(replay, metadata).map_err(|_| Invalid)?;
                VERSION_ADLIB_REPLAY
            }
            (None, None, None) => VERSION_LEGACY,
        };
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
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&metadata_len.to_le_bytes());
        bytes.extend_from_slice(&checksum.finalize().to_le_bytes());
        bytes.append(&mut metadata);
        bytes.extend_from_slice(&cpu);
        Ok(bytes)
    }

    /// Load a saved guest once the entire state and mounted media validate.
    pub fn load_guest(&mut self, bytes: &[u8]) -> Result<(), GuestSnapshotError> {
        self.load_guest_snapshot(bytes, None, &[]).map(drop)
    }

    /// Load the guest and return exactly the requested number of shell file tables.
    pub fn load_guest_with_shell_io(
        &mut self,
        bytes: &[u8],
        streams: &[u8],
        count: usize,
    ) -> Result<Vec<ShellIo>, GuestSnapshotError> {
        self.load_guest_snapshot(bytes, Some((streams, count)), &[])
    }

    /// Load the guest and restore the shared pipe buffers and shell file tables.
    pub fn load_guest_with_pipeline_io(
        &mut self,
        bytes: &[u8],
        io: &[u8],
        stream_count: usize,
        pipeline_count: usize,
    ) -> Result<RestoredPipelineIo, GuestSnapshotError> {
        use GuestSnapshotError::Invalid;
        let io: PipelineIo = decode_extension(io)?;
        if io.pending.len() != pipeline_count {
            return Err(Invalid);
        }
        let pipes = SnapshotPipes::restore(io.buffers).ok_or(Invalid)?;
        let pending = io
            .pending
            .into_iter()
            .map(|id| match id {
                Some(id) => pipes
                    .get(usize::from(id))
                    .map(|pipe| Some(ShellPipe(std::sync::Arc::clone(&pipe.0)))),
                None => Some(None),
            })
            .collect::<Option<_>>()
            .ok_or(Invalid)?;
        let streams = self.load_guest_snapshot(bytes, Some((&io.streams, stream_count)), &pipes)?;
        Ok(RestoredPipelineIo {
            streams,
            pipes: pending,
        })
    }

    fn load_guest_snapshot(
        &mut self,
        bytes: &[u8],
        streams: Option<(&[u8], usize)>,
        pipes: &[ShellPipe],
    ) -> Result<Vec<ShellIo>, GuestSnapshotError> {
        use GuestSnapshotError::{Invalid, MediaChanged};
        if !(HEADER_BYTES..=MAX_GUEST_SNAPSHOT_BYTES).contains(&bytes.len()) || &bytes[..8] != MAGIC
        {
            return Err(Invalid);
        }
        let version = u16::from_le_bytes(bytes[8..10].try_into().unwrap());
        if !matches!(
            version,
            VERSION_LEGACY | VERSION_ADLIB_REPLAY | VERSION_BOOT_MEDIA | VERSION_MIDI
        ) {
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
        let (boot, replay, midi) = match version {
            VERSION_LEGACY if trailing.is_empty() => (None, None, None),
            VERSION_ADLIB_REPLAY => (None, Some(decode_extension::<SynthReplay>(trailing)?), None),
            VERSION_BOOT_MEDIA => {
                let (boot, replay) =
                    decode_extension::<(DiskSnapshot, Option<SynthReplay>)>(trailing)?;
                (Some(boot), replay, None)
            }
            VERSION_MIDI => {
                let (boot, replay, midi) = decode_extension::<(
                    Option<DiskSnapshot>,
                    Option<SynthReplay>,
                    MidiSnapshot<MidiReplay>,
                )>(trailing)?;
                (boot, replay, Some(midi))
            }
            _ => return Err(Invalid),
        };
        if !state.state.is_active()
            || state.elapsed > MAX_ELAPSED
            || state.keys.len() > MAX_KEYS
            || state.line.len() > 255
            || !state.keyboard.valid_snapshot()
            || !state.mouse.valid_snapshot()
            || !state.pit.valid_snapshot(state.elapsed)
        {
            return Err(Invalid);
        }
        if fingerprints(
            &self.mounts,
            self.boot_disk.as_ref().map_or(0, BootDisk::media_bytes),
        )? != state.media
        {
            return Err(MediaChanged);
        }
        let boot_status = match (boot, &self.boot_disk) {
            (None, None) => None,
            (Some(saved), Some(disk)) if disk.snapshot()?.media == saved.media => {
                Some(saved.status)
            }
            _ => return Err(MediaChanged),
        };
        let mut environment = Environment::default();
        environment.set("COMSPEC", "").map_err(|_| Invalid)?;
        environment.set("BLASTER", "").map_err(|_| Invalid)?;
        for (key, value) in state.environment {
            environment.set(&key, &value).map_err(|_| Invalid)?;
        }
        let session_clock =
            clock::SessionClock::from_snapshot(state.session_clock).ok_or(Invalid)?;
        let files = DosFiles::from_snapshot(state.files, &self.mounts, pipes).ok_or(Invalid)?;
        let streams = match streams {
            Some((bytes, count)) => files
                .restore_shell_io(&self.mounts, bytes, count, pipes)
                .ok_or(Invalid)?,
            None => Vec::new(),
        };
        let mut audio =
            audio::Audio::from_snapshot(state.audio, state.elapsed, replay).ok_or(Invalid)?;
        audio.mpu = Mpu401::from_snapshot(midi, &self.audio.mpu, audio.sample)?;
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
        let mut boot_disk = self.boot_disk.take();
        if let Some((disk, status)) = boot_disk.as_mut().zip(boot_status) {
            disk.restore_status(status);
        }
        *self = Self {
            cpu,
            cpu_profile: state.profile,
            video,
            mounts: std::mem::take(&mut self.mounts),
            boot_disk,
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
        Ok(streams)
    }
}

fn decode_extension<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, GuestSnapshotError> {
    let (value, remainder) =
        postcard::take_from_bytes(bytes).map_err(|_| GuestSnapshotError::Invalid)?;
    if !remainder.is_empty() {
        return Err(GuestSnapshotError::Invalid);
    }
    Ok(value)
}

fn fingerprints(
    mounts: &BTreeMap<char, Mount>,
    boot_bytes: u64,
) -> Result<BTreeMap<char, u32>, GuestSnapshotError> {
    let mut total = checked_media_size(0, boot_bytes)?;
    let mut entry_count = 0;
    let mut media = BTreeMap::new();
    for (&drive, mount) in mounts {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(mount.description().as_bytes());
        if let Some(sectors) = mount.iso_sector_count() {
            let bytes = u64::from(sectors) * dosa_mount::ISO_SECTOR_BYTES as u64;
            total = checked_media_size(total, bytes)?;
            hash_iso_sectors(mount, sectors, &mut hasher)?;
            media.insert(drive, hasher.finalize());
            continue;
        }
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
                    total = checked_media_size(total, entry.size)?;
                    let bytes = mount.read(&path)?;
                    if bytes.len() as u64 != entry.size {
                        return Err(GuestSnapshotError::MediaChanged);
                    }
                    hasher.update(&bytes);
                }
            }
        }
        media.insert(drive, hasher.finalize());
    }
    Ok(media)
}

fn checked_media_size(current: u64, bytes: u64) -> Result<u64, GuestSnapshotError> {
    current
        .checked_add(bytes)
        .filter(|&total| total <= MAX_MEDIA_BYTES)
        .ok_or(GuestSnapshotError::Unavailable(
            "mounted media are too large",
        ))
}

fn hash_iso_sectors(
    mount: &Mount,
    sectors: u32,
    hasher: &mut crc32fast::Hasher,
) -> Result<(), MountError> {
    const BATCH: usize = 64;
    const SECTOR_BYTES: usize = dosa_mount::ISO_SECTOR_BYTES;
    hasher.update(&sectors.to_le_bytes());
    let mut bytes = vec![0; BATCH * SECTOR_BYTES];
    for first in (0..sectors).step_by(BATCH) {
        let count = (sectors - first).min(BATCH as u32) as usize;
        let buffer = &mut bytes[..count * SECTOR_BYTES];
        mount.read_iso_sectors(first, buffer)?;
        hasher.update(buffer);
    }
    Ok(())
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
        assert_eq!(&bytes[8..10], &VERSION_LEGACY.to_le_bytes());

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
    fn restores_an_active_adlib_envelope_at_the_same_sample_phase() {
        let directory = Directory::new("save-guest-adlib");
        directory.write("LOOP.COM", &[0xeb, 0xfe]);
        let mut machine = directory.machine();
        machine.launch('C', "LOOP.COM", "").unwrap();
        let started = machine.elapsed();
        let start = started + Duration::from_millis(1);
        let write = |m: &mut Machine, address, value, at| {
            assert!(m.audio.write_port(0x388, address, at, &mut m.cpu));
            assert!(m.audio.write_port(0x389, value, at, &mut m.cpu));
        };
        for (register, value) in [
            (0x20, 1),
            (0x23, 1),
            (0x40, 0),
            (0x43, 0),
            (0x60, 0xf0),
            (0x63, 0xf0),
            (0x80, 0x77),
            (0x83, 0x77),
            (0xa0, 0x98),
            (0xb0, 0x31),
        ] {
            write(&mut machine, register, value, start);
        }
        let change = started + Duration::from_millis(20);
        write(&mut machine, 0xa0, 0xcc, change);
        machine
            .audio
            .advance(started + Duration::from_millis(35), &machine.cpu);
        machine.take_audio();
        machine.paused_since = Some(machine.clock + started + Duration::from_millis(40));
        let state = machine.save_guest().unwrap();
        assert_eq!(&state[8..10], &VERSION_ADLIB_REPLAY.to_le_bytes());

        let mut restored = directory.machine();
        restored.load_guest(&state).unwrap();
        let end = started + Duration::from_millis(46);
        machine.audio.advance(end, &machine.cpu);
        restored.audio.advance(end, &restored.cpu);
        let expected = machine.take_audio();
        assert!(expected.iter().any(|sample| sample.abs() > 0.01));
        assert_eq!(restored.take_audio(), expected);
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
