//! A headless DOS process runtime with a BIOS text and VGA display.

mod adlib;
mod ansi;
mod audio;
pub use audio::AudioLevels;
mod bios;
mod boot_disk;
mod clock;
mod code_page;
mod dma;
mod environment;
mod files;
mod gameport;
pub use files::{DosDevice, matches_dos_pattern};
mod keyboard;
mod loader;
mod mouse;
mod mpu401;
mod mscdex;
mod pic;
mod pit;
mod services;
mod snapshot;
mod sound_blaster;
mod speaker;
mod vga;
mod video;
mod xms;

/// Sample rate of the mono PCM samples produced by [Machine::take_audio].
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;

/// Major and minor version advertised by the DOS services.
pub const DOS_VERSION: [u8; 2] = [5, 0];

#[cfg(test)]
mod tests;

pub use gameport::JoystickState;
pub use mouse::MouseCursor;
pub use snapshot::{GuestSnapshotError, MAX_GUEST_SNAPSHOT_BYTES, RestoredPipelineIo};
pub use vga::{TextFontBanks, VgaTextFonts};
pub use video::{TextFrame, Video};

use bios::{Hook, TIMER_VECTOR};
pub use boot_disk::BootDrive;
pub use clock::DosClock;
pub use code_page::CodePage;
use dosa_cpu::{Cpu, CpuError, Step};
pub use dosa_cpu::{CpuProfile, Registers};
use dosa_mount::{FileWriteOptions, Mount, MountError};
pub use environment::Environment;
use files::DosFiles;
use keyboard::{KEYBOARD_VECTOR, Keyboard, MAX_KEYS};
use loader::{CONVENTIONAL_END, PSP};
use mouse::Mouse;
use pic::Pic;
use pit::Pit;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::time::{Duration, Instant};
use video::Adapter;

const CARRY: u16 = 1;
const ZERO: u16 = 0x40;
const TIMER_SAMPLE_INTERVAL: usize = 256;

#[derive(Debug)]
pub enum DosError {
    InvalidDrive(char),
    UnmountedDrive(char),
    NotOpticalDrive(char),
    InvalidCommandTail,
    Redirection { path: String, code: u16 },
    InvalidEnvironment(&'static str),
    InvalidProgram(&'static str),
    Busy,
    GuestNotPaused,
    BootMedia(std::io::Error),
    ReplaceFloppy(std::io::Error),
    NoBootFloppy,
    Mount(MountError),
    Cpu(CpuError),
    CpuAfterDeniedWrite { error: CpuError, drive: char },
    Interrupt { vector: u8, ax: u16 },
}

impl fmt::Display for DosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDrive(drive) => write!(f, "invalid DOS drive {drive}"),
            Self::UnmountedDrive(drive) => write!(f, "DOS drive {drive}: is unmounted"),
            Self::NotOpticalDrive(drive) => write!(f, "DOS drive {drive}: has no ISO mounted"),
            Self::InvalidCommandTail => write!(
                f,
                "command tail must be at most 126 ASCII bytes including its leading space"
            ),
            Self::Redirection { path, code } => {
                write!(f, "cannot redirect {path}: DOS error {code}")
            }
            Self::InvalidProgram(message) | Self::InvalidEnvironment(message) => {
                f.write_str(message)
            }
            Self::Busy => f.write_str("a DOS program is already running"),
            Self::GuestNotPaused => f.write_str("pause a DOS program before stepping"),
            Self::BootMedia(error) => write!(f, "cannot boot disk image: {error}"),
            Self::ReplaceFloppy(error) => write!(f, "cannot read replacement floppy: {error}"),
            Self::NoBootFloppy => f.write_str("no boot floppy is active"),
            Self::Mount(error) => write!(f, "{error}"),
            Self::Cpu(error) => write!(f, "{error}"),
            Self::CpuAfterDeniedWrite { error, drive } => write!(
                f,
                "{error}\r\nAn earlier write request to {drive}: failed because the drive is read-only."
            ),
            Self::Interrupt { vector, ax } => {
                write!(f, "unsupported interrupt {vector:02X}h (AX={ax:04X}h)")
            }
        }
    }
}

impl std::error::Error for DosError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RunState {
    Idle,
    Running,
    WaitingForKey,
    WaitingForInterrupt,
    Exited(u8),
}

impl RunState {
    fn is_active(self) -> bool {
        matches!(
            self,
            Self::Running | Self::WaitingForKey | Self::WaitingForInterrupt
        )
    }
}

/// A read-only view of the guest at a paused instruction boundary.
pub struct PausedGuest<'a> {
    cpu: &'a Cpu,
    pub state: RunState,
}

impl PausedGuest<'_> {
    pub fn registers(&self) -> Registers {
        self.cpu.regs
    }

    pub fn instruction_pointer(&self) -> u32 {
        self.cpu.instruction_pointer()
    }

    pub fn stack_pointer(&self) -> u32 {
        self.cpu.stack_pointer()
    }

    pub fn source_pointer(&self) -> u32 {
        self.cpu.source_pointer()
    }

    pub fn peek_data(&self, relative: u8) -> Option<u8> {
        self.cpu.peek_data_byte(relative)
    }

    pub fn peek_code(&self, relative: u8) -> Option<u8> {
        self.cpu.peek_code_byte(relative)
    }

    pub fn peek_stack(&self, relative: u8) -> Option<u8> {
        self.cpu.peek_stack_byte(relative)
    }

    pub fn peek(&self, segment: u16, offset: u16) -> Option<u8> {
        self.cpu.peek_u8(segment, offset)
    }
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Pending {
    Bios,
    Console { echo: bool, check_break: bool },
    Dos { ah: u8, al: u8 },
    Line,
    HandleRead,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct PendingCall {
    request: Pending,
    address: (u16, u16),
    stack: (u16, u16),
}

impl PendingCall {
    fn new(request: Pending, cpu: &Cpu) -> Self {
        Self {
            request,
            address: (cpu.regs.cs, cpu.regs.ip),
            stack: (cpu.regs.ss, cpu.regs.sp),
        }
    }

    fn returned(self, cpu: &Cpu) -> bool {
        self.address == (cpu.regs.cs, cpu.regs.ip) && self.stack == (cpu.regs.ss, cpu.regs.sp)
    }
}

/// Standard handles prepared for one command.
pub struct ShellIo {
    files: DosFiles,
}

/// Buffered data passed between two sequential DOS shell commands.
#[derive(Default)]
pub struct ShellPipe(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl ShellIo {
    pub fn attach_pipe(&mut self, pipe: &ShellPipe, writable: bool) {
        self.files
            .attach_pipe(std::sync::Arc::clone(&pipe.0), writable);
    }
}

pub enum ShellInput {
    Console,
    Eof,
    Byte(u8),
}

/// Limits for loading a program into the built-in DOS upper-memory region.
#[derive(Clone, Copy, Debug, Default)]
pub struct UpperMemoryRequest {
    pub minimum_bytes: u32,
    pub shrink: bool,
    pub allow_conventional_allocations: bool,
}

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
struct UpperProgram {
    end: u16,
    allow_conventional_allocations: bool,
}

impl UpperProgram {
    const MIN_END: u16 = xms::UPPER_START + 0x11;

    fn for_request(profile: CpuProfile, request: UpperMemoryRequest) -> Option<Self> {
        let paragraphs = u16::try_from(request.minimum_bytes.div_ceil(16)).ok()?;
        let minimum_end = xms::UPPER_START.checked_add(paragraphs)?;
        if profile == CpuProfile::I8086 || minimum_end > xms::UPPER_END {
            return None;
        }
        Some(Self {
            end: if request.shrink {
                minimum_end.max(Self::MIN_END)
            } else {
                xms::UPPER_END
            },
            allow_conventional_allocations: request.allow_conventional_allocations,
        })
    }
}

#[derive(Clone, Copy, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct MemoryStrategy {
    code: u8,
    linked: bool,
}

impl MemoryStrategy {
    fn for_program(program: Option<UpperProgram>) -> Self {
        match program {
            Some(program) => Self {
                code: if program.allow_conventional_allocations {
                    0x80
                } else {
                    0x40
                },
                linked: true,
            },
            None => Self::default(),
        }
    }

    fn valid_code(code: u8) -> bool {
        matches!(code, 0..=2 | 0x40..=0x42 | 0x80..=0x82)
    }
}

pub struct Machine {
    cpu: Cpu,
    cpu_profile: CpuProfile,
    video: Adapter,
    mounts: BTreeMap<char, Mount>,
    iso_media_changed: u32,
    boot_disk: Option<boot_disk::BootDisk>,
    environment: Environment,
    files: DosFiles,
    state: RunState,
    pending: Vec<PendingCall>,
    keys: VecDeque<(u8, u8)>,
    keyboard: Keyboard,
    fast_a20: bool,
    xms: xms::Xms,
    gameport: gameport::GamePort,
    mouse: Mouse,
    pic: Pic,
    pit: Pit,
    audio: audio::Audio,
    modifiers: u8,
    locks: u8,
    output: Vec<u8>,
    clock: Instant,
    session_clock: clock::SessionClock,
    verify_writes: bool,
    extended_break: bool,
    paused_since: Option<Instant>,
    dta: (u16, u16),
    upper_program: Option<UpperProgram>,
    memory_strategy: MemoryStrategy,
    session_memory_strategy: MemoryStrategy,
    blocks: BTreeMap<u16, u16>,
    line: Vec<u8>,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    pub fn new() -> Self {
        let mut cpu = Cpu::new();
        let mut video = Adapter::new();
        video::initialize(&mut cpu, &mut video);
        bios::install(&mut cpu, 0);
        xms::Xms::install(&mut cpu);
        Mouse::install(&mut cpu);
        Self {
            cpu,
            cpu_profile: CpuProfile::default(),
            video,
            mounts: BTreeMap::new(),
            iso_media_changed: 0,
            boot_disk: None,
            environment: Environment::default(),
            files: DosFiles::new('C', String::new()),
            state: RunState::Idle,
            pending: Vec::new(),
            keys: VecDeque::new(),
            keyboard: Keyboard::default(),
            fast_a20: false,
            xms: xms::Xms::default(),
            gameport: gameport::GamePort::default(),
            mouse: Mouse::default(),
            pic: Pic::default(),
            pit: Pit::new(0),
            audio: audio::Audio::new(Duration::ZERO),
            modifiers: 0,
            locks: 0,
            output: Vec::new(),
            clock: Instant::now(),
            session_clock: clock::SessionClock::default(),
            verify_writes: false,
            extended_break: false,
            paused_since: None,
            dta: (PSP, 0x80),
            upper_program: None,
            memory_strategy: MemoryStrategy::default(),
            session_memory_strategy: MemoryStrategy::default(),
            blocks: BTreeMap::new(),
            line: Vec::new(),
        }
    }

    pub fn mount(&mut self, drive: char, mount: Mount) -> Result<(), DosError> {
        if self.state.is_active() {
            return Err(DosError::Busy);
        }
        let drive = normalize_drive(drive)?;
        self.mounts.insert(drive, mount);
        self.iso_media_changed &= !(1 << (drive as u8 - b'A'));
        Ok(())
    }

    /// Replace a mounted CD-ROM image while preserving its drive letter and guest execution.
    pub fn swap_iso(&mut self, drive: char, mount: Mount) -> Result<(), DosError> {
        let drive = normalize_drive(drive)?;
        if self
            .drive(drive)
            .and_then(Mount::iso_sector_count)
            .is_none()
        {
            return Err(DosError::NotOpticalDrive(drive));
        }
        if mount.iso_sector_count().is_none() {
            return Err(DosError::Mount(MountError::UnsupportedSource));
        }
        self.mounts.insert(drive, mount);
        self.iso_media_changed |= 1 << (drive as u8 - b'A');
        self.files.reset_media_directory(drive);
        Ok(())
    }

    /// Reset the guest CPU and devices while retaining media, the profile, and the shell environment.
    pub fn reset(&mut self) {
        let ansi = self.video.ansi.is_some();
        let audio_levels = self.audio.levels;
        let mut mpu = std::mem::take(&mut self.audio.mpu);
        mpu.reset();
        let mut gameport = std::mem::take(&mut self.gameport);
        gameport.reset_timers();
        *self = Self {
            mounts: std::mem::take(&mut self.mounts),
            environment: std::mem::take(&mut self.environment),
            cpu_profile: self.cpu_profile,
            session_clock: self.session_clock,
            gameport,
            ..Self::new()
        };
        self.set_ansi_text(ansi);
        self.audio.mpu = mpu;
        self.audio.set_levels(audio_levels);
    }

    /// Configure the MPU-401 UART and its SoundFont synthesizer while the guest is idle.
    pub fn load_midi_soundfont(
        &mut self,
        source: &mut impl std::io::Read,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if self.state.is_active() {
            return Err(DosError::Busy.into());
        }
        self.audio.mpu.load(source)
    }

    /// Enable DOS console processing of ANSI text escape sequences.
    pub fn set_ansi_text(&mut self, enabled: bool) {
        if enabled != self.video.ansi.is_some() {
            self.video.ansi = enabled.then(ansi::Ansi::default);
        }
    }

    /// Read the current session date and time without changing the host clock.
    pub fn clock_snapshot(&self) -> DosClock {
        self.session_clock.now()
    }

    /// Return the DOS time for a mounted file when the session clock was adjusted.
    pub fn adjusted_file_time(&self) -> Option<std::time::SystemTime> {
        self.session_clock.adjusted_file_time()
    }

    /// Whether DOS requests read-back verification for mounted file writes.
    pub fn verify_writes(&self) -> bool {
        self.verify_writes
    }

    pub fn set_verify_writes(&mut self, enabled: bool) {
        self.verify_writes = enabled;
    }

    /// Whether DOS checks for Ctrl+C during non-console system calls.
    pub fn extended_break(&self) -> bool {
        self.extended_break
    }

    pub fn set_extended_break(&mut self, enabled: bool) {
        self.extended_break = enabled;
    }

    pub fn code_page(&self) -> CodePage {
        self.video.code_page
    }

    pub fn set_code_page(&mut self, page: CodePage) {
        self.video.set_code_page(&mut self.cpu, page);
    }

    pub fn set_date(&mut self, year: u16, month: u8, day: u8) -> bool {
        self.session_clock.set_date(year, month, day)
    }

    pub fn set_time(&mut self, hour: u8, minute: u8, second: u8, hundredths: u8) -> bool {
        self.session_clock
            .set_time(hour, minute, second, hundredths)
    }

    pub fn environment(&self) -> &Environment {
        &self.environment
    }

    pub fn environment_mut(&mut self) -> &mut Environment {
        &mut self.environment
    }

    pub fn drive(&self, drive: char) -> Option<&Mount> {
        self.mounts.get(&drive.to_ascii_uppercase())
    }

    /// Last drive that denied a guest write-open or create because of its mount mode.
    pub fn denied_write_drive(&self) -> Option<char> {
        self.files.denied_write_drive
    }

    pub fn unmount(&mut self, drive: char) -> Result<(), DosError> {
        if self.state.is_active() {
            return Err(DosError::Busy);
        }
        let drive = normalize_drive(drive)?;
        self.mounts
            .remove(&drive)
            .ok_or(DosError::UnmountedDrive(drive))?;
        self.iso_media_changed &= !(1 << (drive as u8 - b'A'));
        Ok(())
    }

    pub fn set_cpu_profile(&mut self, profile: CpuProfile) -> Result<(), DosError> {
        if self.state.is_active() {
            return Err(DosError::Busy);
        }
        self.cpu_profile = profile;
        Ok(())
    }

    pub fn cpu_profile(&self) -> CpuProfile {
        self.cpu_profile
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused && self.state.is_active() {
            self.paused_since.get_or_insert_with(Instant::now);
        } else if !paused && let Some(since) = self.paused_since.take() {
            self.clock += since.elapsed();
        }
    }

    pub fn paused_guest(&self) -> Option<PausedGuest<'_>> {
        self.paused_since.map(|_| PausedGuest {
            cpu: &self.cpu,
            state: self.state,
        })
    }

    pub fn step_paused(&mut self) -> Result<RunState, DosError> {
        if self.paused_since.is_none() || !self.state.is_active() {
            return Err(DosError::GuestNotPaused);
        }
        self.run_budget(1)
    }

    fn elapsed(&self) -> Duration {
        self.paused_since.map_or_else(
            || self.clock.elapsed(),
            |paused| paused.duration_since(self.clock),
        )
    }

    pub fn launch(&mut self, drive: char, path: &str, args: &str) -> Result<(), DosError> {
        let drive = normalize_drive(drive)?;
        let cwd = executable_parent(path).replace('/', "\\");
        self.launch_with_files(drive, path, args, DosFiles::new(drive, cwd), None)
    }

    /// Start a program while inheriting the shell's default drive and per-drive directories.
    pub fn launch_from_shell(
        &mut self,
        drive: char,
        path: &str,
        args: &str,
        current_drive: char,
        directories: &BTreeMap<char, String>,
    ) -> Result<(), DosError> {
        let files =
            DosFiles::with_directories(normalize_drive(current_drive)?, directories.clone());
        self.launch_with_files(normalize_drive(drive)?, path, args, files, None)
    }

    /// Open shell stream redirections using the DOS working directories and mounted drives.
    pub fn prepare_shell_io(
        &self,
        current_drive: char,
        directories: &BTreeMap<char, String>,
        inherited: Option<&ShellIo>,
        input: Option<&str>,
        output: Option<(&str, bool)>,
    ) -> Result<ShellIo, DosError> {
        let mut files =
            DosFiles::with_directories(normalize_drive(current_drive)?, directories.clone());
        if let Some(parent) = inherited {
            files.inherit_standard_handles(&parent.files);
        }
        let mut io = ShellIo { files };
        self.redirect_shell_io(&mut io, input, output)?;
        Ok(io)
    }

    pub fn redirect_shell_io(
        &self,
        io: &mut ShellIo,
        input: Option<&str>,
        output: Option<(&str, bool)>,
    ) -> Result<(), DosError> {
        let files = &mut io.files;
        if let Some(path) = input {
            let redirect = (|| {
                let handle = files.open(&self.mounts, path, 0)?;
                files.duplicate_to(handle, 0)?;
                files.close(handle)
            })();
            redirect.map_err(|code| DosError::Redirection {
                path: path.to_owned(),
                code,
            })?;
        }
        if let Some((path, append)) = output {
            let redirect = (|| {
                let handle = if append {
                    match files.open(&self.mounts, path, 1) {
                        Ok(handle) => {
                            files.seek(handle, 2, 0)?;
                            handle
                        }
                        Err(2) => files.create(&self.mounts, path, self.adjusted_file_time())?,
                        Err(code) => return Err(code),
                    }
                } else {
                    files.create(&self.mounts, path, self.adjusted_file_time())?
                };
                files.duplicate_to(handle, 1)?;
                files.close(handle)
            })();
            redirect.map_err(|code| DosError::Redirection {
                path: path.to_owned(),
                code,
            })?;
        }
        Ok(())
    }

    /// Write builtin output to the redirected handle. Return false for display output.
    pub fn write_shell_stdout(&self, io: &mut ShellIo, bytes: &[u8]) -> Result<bool, DosError> {
        let write = || FileWriteOptions {
            modified: self.adjusted_file_time(),
            verify: self.verify_writes(),
        };
        let result = match io.files.output_target(1) {
            Ok(files::HandleTarget::Console) => return Ok(false),
            Ok(files::HandleTarget::Null) => Ok(()),
            Ok(files::HandleTarget::File(handle)) if !bytes.is_empty() => {
                io.files.write(handle, bytes, write())
            }
            Ok(files::HandleTarget::File(_)) => Ok(()),
            Err(code) => Err(code),
        };
        result.map(|()| true).map_err(|code| DosError::Redirection {
            path: "stdout".into(),
            code,
        })
    }

    /// Read one byte of redirected builtin input without consuming host keyboard events.
    pub fn read_shell_stdin(&self, io: &mut ShellIo) -> Result<ShellInput, DosError> {
        let result = match io.files.input_target(0) {
            Ok(files::HandleTarget::Console) => Ok(ShellInput::Console),
            Ok(files::HandleTarget::Null) => Ok(ShellInput::Eof),
            Ok(files::HandleTarget::File(handle)) => io.files.read(handle, 1).map(|bytes| {
                bytes
                    .first()
                    .copied()
                    .map_or(ShellInput::Eof, ShellInput::Byte)
            }),
            Err(code) => Err(code),
        };
        result.map_err(|code| DosError::Redirection {
            path: "stdin".into(),
            code,
        })
    }

    /// Start a shell program with its prepared standard handles.
    pub fn launch_with_shell_io(
        &mut self,
        drive: char,
        path: &str,
        args: &str,
        io: ShellIo,
    ) -> Result<(), DosError> {
        self.launch_with_files(normalize_drive(drive)?, path, args, io.files, None)
    }

    /// Start a program in upper memory when its image and memory request fit.
    pub fn launch_high_with_shell_io(
        &mut self,
        drive: char,
        path: &str,
        args: &str,
        io: ShellIo,
        request: UpperMemoryRequest,
    ) -> Result<(), DosError> {
        let upper_program = UpperProgram::for_request(self.cpu_profile, request);
        self.launch_with_files(normalize_drive(drive)?, path, args, io.files, upper_program)
    }

    fn launch_with_files(
        &mut self,
        drive: char,
        path: &str,
        args: &str,
        files: DosFiles,
        upper_program: Option<UpperProgram>,
    ) -> Result<(), DosError> {
        if self.state.is_active() {
            return Err(DosError::Busy);
        }
        let mount = self.drive(drive).ok_or(DosError::UnmountedDrive(drive))?;
        let entry = mount.metadata(path).map_err(DosError::Mount)?;
        let bytes = mount.read(path).map_err(DosError::Mount)?;
        let parent = executable_parent(path).replace('/', "\\");
        let name = if parent.is_empty() {
            format!("{drive}:\\{}", entry.name)
        } else {
            format!("{drive}:\\{parent}\\{}", entry.name)
        };
        let load = |upper_program| {
            loader::load(
                &bytes,
                &name,
                args,
                self.cpu_profile,
                &self.environment,
                upper_program,
            )
        };
        let (mut cpu, upper_program) = match upper_program
            .and_then(|upper| load(Some(upper)).ok().map(|cpu| (cpu, Some(upper))))
        {
            Some(loaded) => loaded,
            None => (load(None)?, None),
        };
        cpu.load(cpu.regs.ds, 0x18, &files.psp_handles());
        let mut strategy = if upper_program.is_some() {
            MemoryStrategy::for_program(upper_program)
        } else {
            self.session_memory_strategy
        };
        strategy.linked &= cpu.has_a20();
        self.start(cpu, files, upper_program, strategy);
        Ok(())
    }

    /// Whether the current guest is running from a raw BIOS boot disk.
    pub fn is_booting_from_disk(&self) -> bool {
        self.boot_disk.is_some()
    }

    /// Whether the guest has a raw floppy in its BIOS boot drive.
    pub fn has_boot_floppy(&self) -> bool {
        self.boot_disk
            .as_ref()
            .is_some_and(|disk| disk.drive() == BootDrive::Floppy)
    }

    /// Change the read-only floppy in BIOS drive A: while the guest is active.
    pub fn swap_boot_floppy(&mut self, file: std::fs::File) -> Result<(), DosError> {
        let disk = self
            .boot_disk
            .as_mut()
            .filter(|disk| disk.drive() == BootDrive::Floppy)
            .ok_or(DosError::NoBootFloppy)?;
        disk.replace_floppy(file).map_err(DosError::ReplaceFloppy)?;
        bios::signal_floppy_change(&mut self.cpu);
        Ok(())
    }

    /// Load and execute the first sector of a read-only raw disk image.
    pub fn boot(&mut self, file: std::fs::File, drive: BootDrive) -> Result<(), DosError> {
        self.start_boot_disk(file, drive, false)
    }

    /// Boot an explicitly writable raw hard disk. The file must be open for read and write.
    pub fn boot_writable_hard_disk(&mut self, file: std::fs::File) -> Result<(), DosError> {
        self.start_boot_disk(file, BootDrive::HardDisk, true)
    }

    fn start_boot_disk(
        &mut self,
        file: std::fs::File,
        drive: BootDrive,
        writable: bool,
    ) -> Result<(), DosError> {
        if self.state.is_active() {
            return Err(DosError::Busy);
        }
        let (disk, sector) =
            boot_disk::BootDisk::open(file, drive, writable).map_err(DosError::BootMedia)?;
        let mut cpu = Cpu::with_profile(self.cpu_profile);
        cpu.regs.ip = 0x7c00;
        cpu.regs.sp = 0x7c00;
        cpu.regs.dx = u16::from(drive.number());
        cpu.regs.flags |= 0x0200;
        cpu.load(0, 0x7c00, &sector);
        self.start(
            cpu,
            DosFiles::new('C', String::new()),
            None,
            MemoryStrategy::default(),
        );
        disk.install_data(&mut self.cpu);
        self.boot_disk = Some(disk);
        Ok(())
    }

    fn start(
        &mut self,
        mut cpu: Cpu,
        files: DosFiles,
        upper_program: Option<UpperProgram>,
        memory_strategy: MemoryStrategy,
    ) {
        let mut video = Adapter::new();
        video::initialize(&mut cpu, &mut video);
        let elapsed = self.clock.elapsed();
        let now = pit::ticks(elapsed);
        bios::install(&mut cpu, now);
        if cpu.has_a20() {
            xms::Xms::install(&mut cpu);
        }
        Mouse::install(&mut cpu);
        video.copy_console(&self.video, &self.cpu, &mut cpu);
        self.cpu = cpu;
        self.video = video;
        self.boot_disk = None;
        self.files = files;
        self.pending.clear();
        self.keys.clear();
        self.keyboard = Keyboard::default();
        self.fast_a20 = false;
        self.xms = xms::Xms::default();
        self.upper_program = upper_program;
        self.memory_strategy = memory_strategy;
        self.gameport.reset_timers();
        self.mouse = Mouse::default();
        self.pic = Pic::default();
        self.pit = Pit::new(now);
        self.audio.restart(elapsed);
        self.modifiers = 0;
        self.locks = 0;
        self.output.clear();
        let psp = self.psp();
        let end = upper_program.map_or(CONVENTIONAL_END, |upper| upper.end);
        self.dta = (psp, 0x80);
        self.blocks = BTreeMap::from([(psp, end - psp)]);
        self.refresh_dos_umb_reservation();
        self.line.clear();
        self.state = RunState::Running;
    }

    fn psp(&self) -> u16 {
        if self.upper_program.is_some() {
            xms::UPPER_START
        } else {
            PSP
        }
    }

    /// Update connector A (0) or B (1). Pass `None` when the controller disconnects.
    pub fn set_joystick(&mut self, connector: usize, state: Option<JoystickState>) {
        self.gameport.set(connector, state);
    }

    pub fn key(&mut self, ascii: u8, scan: u8) {
        let guest_handler = self.guest_keyboard_handler();
        let modifier = self.update_modifier(scan, true);
        let mut bios_key = (!modifier && (ascii != 0 || scan != 0)).then_some((ascii, scan));
        if !guest_handler || scan == 0 {
            self.queue_bios_key(bios_key.take());
        }
        self.keyboard.push(scan, guest_handler, bios_key);
    }

    pub fn key_up(&mut self, scan: u8) {
        if scan != 0 {
            self.update_modifier(scan, false);
            let guest_handler = self.guest_keyboard_handler();
            self.keyboard.push(scan | 0x80, guest_handler, None);
        }
    }

    /// Queue a pasted BIOS keystroke when the previous input has been consumed.
    pub fn paste_key(&mut self, ascii: u8, scan: u8) -> bool {
        if !self.state.is_active() || !self.keys.is_empty() {
            return false;
        }
        self.queue_bios_key(Some((ascii, scan)));
        true
    }

    fn guest_keyboard_handler(&self) -> bool {
        !bios::default_keyboard(&self.cpu) && self.has_interrupt_handler(KEYBOARD_VECTOR)
    }

    fn queue_bios_key(&mut self, key: Option<(u8, u8)>) {
        if let Some(key) = key
            && self.keys.len() < MAX_KEYS
        {
            self.keys.push_back(key);
        }
    }

    fn update_modifier(&mut self, scan: u8, pressed: bool) -> bool {
        let mask = match scan {
            0x36 => 0x01, // right shift
            0x2a => 0x02, // left shift
            0x1d => 0x04, // control
            0x38 => 0x08, // alt
            0x46 => 0x10, // scroll lock
            0x45 => 0x20, // num lock
            0x3a => 0x40, // caps lock
            _ => return false,
        };
        if pressed {
            if self.modifiers & mask == 0 && mask & 0x70 != 0 {
                self.locks ^= mask;
            }
            self.modifiers |= mask;
        } else {
            self.modifiers &= !mask;
        }
        self.cpu
            .write_u8(0x40, 0x17, self.modifiers & 0x0f | self.locks);
        self.cpu.write_u8(
            0x40,
            0x18,
            self.modifiers & 0x70 | (self.modifiers & 0x0c) >> 2,
        );
        true
    }

    fn has_interrupt_handler(&self, vector: u8) -> bool {
        let entry = u16::from(vector) * 4;
        self.cpu.read_u16(0, entry) != 0 || self.cpu.read_u16(0, entry + 2) != 0
    }

    pub fn video(&self) -> Video<'_> {
        self.video.frame(&self.cpu)
    }

    /// Whether the running guest has called the integrated DOS mouse driver.
    pub fn mouse_active(&self) -> bool {
        self.mouse.active() && self.state.is_active()
    }

    /// Update the integrated mouse from coordinates normalized to the displayed viewport.
    pub fn mouse_move(&mut self, x: u16, y: u16) {
        self.mouse.move_to(x, y);
    }

    /// Set the left, right, and middle button bits, in that order from bit 0.
    pub fn mouse_buttons(&mut self, mask: u8) {
        self.mouse.buttons(mask);
    }

    /// Return the guest cursor overlay without altering the guest video memory.
    pub fn mouse_cursor(&self) -> Option<MouseCursor> {
        if self.mouse_active() {
            self.mouse.cursor()
        } else {
            None
        }
    }

    fn mouse_interrupt(&mut self) {
        let bounds = match self.video() {
            Video::Text(frame) => (frame.columns * 8, frame.rows * 8),
            Video::Graphics { width, height, .. } => (
                width.max(640).min(u16::MAX.into()) as u16,
                height.min(u16::MAX.into()) as u16,
            ),
        };
        self.mouse.interrupt(&mut self.cpu, bounds);
    }

    pub fn write_console(&mut self, bytes: &[u8]) {
        self.video.write_dos(&mut self.cpu, bytes);
    }

    /// Erase the character before the shell cursor, including across a wrapped row.
    pub fn erase_console_character(&mut self) {
        self.video.erase_previous(&mut self.cpu);
    }

    pub fn clear_console(&mut self) {
        self.video.clear(&mut self.cpu);
    }

    /// Select a shell text geometry and retain it across guest programs.
    pub fn set_text_rows(&mut self, rows: u8) -> bool {
        if self.state.is_active() {
            return false;
        }
        self.video.set_console_rows(&mut self.cpu, rows)
    }

    /// Select a supported host console geometry while the guest is idle.
    pub fn set_text_geometry(&mut self, columns: u16, rows: u8) -> bool {
        if self.state.is_active() {
            return false;
        }
        self.video
            .set_console_geometry(&mut self.cpu, columns, rows)
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// Drain mono PC audio at 48 kHz. The queue retains at most 200 milliseconds.
    pub fn take_audio(&mut self) -> Vec<f32> {
        self.audio.drain()
    }

    /// Adjust the host mix without modifying the guest audio devices.
    pub fn set_audio_levels(&mut self, levels: AudioLevels) {
        self.audio.set_levels(levels);
    }

    pub fn stop(&mut self) {
        if self.state.is_active() {
            self.exit(3);
        }
    }

    pub fn run(&mut self, budget: usize) -> Result<RunState, DosError> {
        if budget == 0 || self.paused_since.is_some() || !self.state.is_active() {
            return Ok(self.state);
        }
        self.run_budget(budget)
    }

    fn run_budget(&mut self, budget: usize) -> Result<RunState, DosError> {
        self.state = RunState::Running;
        self.advance_timers();
        for instruction in 0..budget {
            if !matches!(self.state, RunState::Running | RunState::WaitingForKey) {
                break;
            }
            if instruction != 0 && instruction % TIMER_SAMPLE_INTERVAL == 0 {
                self.advance_timers();
            }
            self.poll_pending_call();
            if self.state != RunState::Running {
                break;
            }
            if let Err(error) = self.step() {
                self.exit(1);
                return Err(error);
            }
        }
        self.video.refresh(&self.cpu);
        Ok(self.state)
    }

    fn advance_timers(&mut self) {
        let elapsed = self.elapsed();
        self.pit.advance(pit::ticks(elapsed));
        self.audio.advance(elapsed, &self.cpu);
    }

    fn poll_pending_call(&mut self) {
        let Some(pending) = self.pending.last().copied() else {
            return;
        };
        if !pending.returned(&self.cpu) {
            return;
        }
        self.pending.pop();
        self.state = RunState::Running;
        self.resume(pending.request);
        // A waiting BIOS/DOS call must let the guest handle IRQs. The
        // continuation remains suspended until its stack and IP return.
        if self.state == RunState::WaitingForKey && self.service_hardware_interrupt() {
            self.state = RunState::Running;
        }
    }

    fn step(&mut self) -> Result<(), DosError> {
        if !self.cpu.protected_mode() {
            let interrupts_enabled =
                self.cpu.can_accept_maskable_interrupt() && !self.cpu.is_halted();
            self.mouse
                .dispatch_callback(&mut self.cpu, interrupts_enabled);
        }
        self.service_hardware_interrupt();
        if !self.cpu.protected_mode() {
            self.run_firmware_hooks();
        }
        let step = self
            .cpu
            .step()
            .map_err(|error| match self.files.denied_write_drive {
                Some(drive) => DosError::CpuAfterDeniedWrite { error, drive },
                None => DosError::Cpu(error),
            })?;
        match step {
            Step::Continue => {}
            Step::Interrupt(vector) => self.interrupt(vector)?,
            Step::PortRead { port, width } => {
                let elapsed = self.elapsed();
                self.audio.advance(elapsed, &self.cpu);
                let mut value = 0;
                for byte in 0..width {
                    let port = port.wrapping_add(u16::from(byte));
                    let requests = self.irq_requests();
                    let mut part = self
                        .pic
                        .port_read(port, requests)
                        .or_else(|| self.audio.read_port(port, elapsed))
                        .or_else(|| self.pit.port_read(port))
                        .or_else(|| {
                            (port == 0x92 && self.cpu.has_a20())
                                .then_some(u8::from(self.fast_a20) << 1)
                        })
                        .or_else(|| self.keyboard.port_read(port))
                        .or_else(|| (port == gameport::PORT).then(|| self.gameport.read(elapsed)))
                        .or_else(|| self.video.port_read(port))
                        .unwrap_or(0xff);
                    if port == 0x61 {
                        part = (part & !0x10) | (u8::from(self.pit.refresh_toggle()) << 4);
                    }
                    value |= u32::from(part) << (u32::from(byte) * 8);
                }
                self.cpu.complete_port_read(width, value);
            }
            Step::PortWrite { port, width, value } => {
                for byte in 0..width {
                    let port = port.wrapping_add(u16::from(byte));
                    let value = (value >> (byte * 8)) as u8;
                    let elapsed = self.elapsed();
                    self.audio.advance(elapsed, &self.cpu);
                    if port == 0x92 && self.cpu.has_a20() {
                        self.fast_a20 = value & 2 != 0;
                        self.refresh_a20();
                        continue;
                    }
                    if port == gameport::PORT {
                        self.gameport.strobe(elapsed);
                        continue;
                    }
                    if !self.audio.write_port(port, value, elapsed, &mut self.cpu)
                        && !self.pic.port_write(port, value)
                        && !self.pit.port_write(port, value)
                    {
                        if self.keyboard.port_write(port, value) {
                            if port == 0x60 {
                                self.refresh_a20();
                            }
                        } else {
                            self.video.port_write(port, value);
                        }
                    }
                }
            }
            Step::Halt => self.state = RunState::WaitingForInterrupt,
        }
        Ok(())
    }

    fn refresh_a20(&mut self) {
        self.cpu
            .set_a20(self.fast_a20 || self.keyboard.a20_enabled() || self.xms.a20_enabled());
    }

    fn xms_available(&self) -> bool {
        self.cpu.has_a20() && self.boot_disk.is_none()
    }

    fn xms_call(&mut self) {
        let function = (self.cpu.regs.ax >> 8) as u8;
        if !matches!(function, 3..=6) {
            self.xms.call(&mut self.cpu);
            return;
        }
        let result = self.xms.update_a20(function);
        self.refresh_a20();
        let result = result.and_then(|()| {
            let disabling = function == 4 || function == 6 && !self.xms.a20_enabled();
            if disabling && self.cpu.a20_enabled() {
                Err(0x94)
            } else {
                Ok(())
            }
        });
        xms::Xms::complete(&mut self.cpu, result);
    }

    fn run_firmware_hooks(&mut self) {
        if xms::Xms::at_entry(&self.cpu) && self.xms_available() {
            self.xms_call();
        }
        match bios::hook(&self.cpu) {
            Some(Hook::Tick) => bios::advance_timer(&mut self.cpu),
            Some(Hook::TimerEoi) => self.pic.end_irq(0),
            Some(Hook::Keyboard) => {
                let key = self.keyboard.read_bios_key();
                self.queue_bios_key(key);
                self.pic.end_irq(1);
            }
            None => {}
        }
        if self.mouse.at_driver_entry(&self.cpu) {
            self.mouse_interrupt();
        }
    }

    fn irq_requests(&self) -> u8 {
        u8::from(self.pit.irq_requested())
            | (u8::from(self.keyboard.irq_requested()) << 1)
            | (u8::from(self.audio.irq_requested()) << sound_blaster::IRQ)
    }

    fn service_hardware_interrupt(&mut self) -> bool {
        if !self.cpu.can_accept_maskable_interrupt() {
            return false;
        }
        let Some(irq) = self.pic.next_irq(self.irq_requests()) else {
            return false;
        };
        let vector = TIMER_VECTOR + irq;
        let protected = self.cpu.protected_mode();
        if irq == 0 && !protected {
            self.pit.acknowledge_irq();
        }
        if (protected || self.has_interrupt_handler(vector))
            && self.cpu.enter_interrupt(vector).is_some()
        {
            if irq == 0 && protected {
                self.pit.acknowledge_irq();
            }
            if vector == KEYBOARD_VECTOR {
                self.keyboard.acknowledge_irq();
            }
            if irq == sound_blaster::IRQ {
                self.audio.acknowledge_irq();
            }
            self.pic.acknowledge(irq);
            return true;
        }
        false
    }

    fn exit(&mut self, code: u8) {
        self.set_paused(false);
        self.state = RunState::Exited(code);
        self.boot_disk = None;
        self.pending.clear();
        self.files.close_all();
        self.audio.mpu.reset();
        self.video.restore_text(&mut self.cpu);
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
        self.write_console(bytes);
    }

    fn success(&mut self) {
        self.cpu.regs.flags &= !CARRY;
    }

    fn failure(&mut self, code: u16) {
        self.cpu.regs.flags |= CARRY;
        self.cpu.regs.ax = code;
    }
}

fn executable_parent(path: &str) -> &str {
    path.rsplit_once(['\\', '/'])
        .map_or("", |(parent, _)| parent.trim_matches(['\\', '/']))
}

fn normalize_drive(drive: char) -> Result<char, DosError> {
    if drive.is_ascii_alphabetic() {
        Ok(drive.to_ascii_uppercase())
    } else {
        Err(DosError::InvalidDrive(drive))
    }
}
