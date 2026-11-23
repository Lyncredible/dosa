//! A headless DOS process runtime with a BIOS text and VGA display.

mod files;
mod loader;
mod services;
mod vga;
mod video;

#[cfg(test)]
mod tests;

pub use video::{TextFrame, Video};

use dosa_cpu::{Cpu, CpuError, Step};
use dosa_mount::{Mount, MountError};
use files::DosFiles;
use loader::{CONVENTIONAL_END, PSP};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::time::Instant;
use video::Adapter;

const CARRY: u16 = 1;
const ZERO: u16 = 0x40;
const MAX_KEYS: usize = 32;

#[derive(Debug)]
pub enum DosError {
    InvalidDrive(char),
    UnmountedDrive(char),
    InvalidCommandTail,
    InvalidProgram(&'static str),
    Busy,
    Mount(MountError),
    Cpu(CpuError),
    Interrupt { vector: u8, ax: u16 },
}

impl fmt::Display for DosError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDrive(drive) => write!(f, "invalid DOS drive {drive}"),
            Self::UnmountedDrive(drive) => write!(f, "DOS drive {drive}: is unmounted"),
            Self::InvalidCommandTail => write!(
                f,
                "command tail must be at most 126 ASCII bytes including its leading space"
            ),
            Self::InvalidProgram(message) => f.write_str(message),
            Self::Busy => f.write_str("a DOS program is already running"),
            Self::Mount(error) => write!(f, "{error}"),
            Self::Cpu(error) => write!(f, "{error}"),
            Self::Interrupt { vector, ax } => {
                write!(f, "unsupported interrupt {vector:02X}h (AX={ax:04X}h)")
            }
        }
    }
}

impl std::error::Error for DosError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunState {
    Idle,
    Running,
    WaitingForKey,
    Exited(u8),
}

#[derive(Clone, Copy)]
enum Pending {
    Bios,
    Console { echo: bool },
    Line,
    HandleRead,
}

pub struct Machine {
    cpu: Cpu,
    video: Adapter,
    mounts: BTreeMap<char, Mount>,
    files: DosFiles,
    state: RunState,
    pending: Option<Pending>,
    keys: VecDeque<(u8, u8)>,
    scancodes: VecDeque<u8>,
    modifiers: u8,
    locks: u8,
    output: Vec<u8>,
    clock: Instant,
    dta: (u16, u16),
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
        Self {
            cpu,
            video,
            mounts: BTreeMap::new(),
            files: DosFiles::new('C', String::new()),
            state: RunState::Idle,
            pending: None,
            keys: VecDeque::new(),
            scancodes: VecDeque::new(),
            modifiers: 0,
            locks: 0,
            output: Vec::new(),
            clock: Instant::now(),
            dta: (PSP, 0x80),
            blocks: BTreeMap::new(),
            line: Vec::new(),
        }
    }

    pub fn mount(&mut self, drive: char, mount: Mount) -> Result<(), DosError> {
        if matches!(self.state, RunState::Running | RunState::WaitingForKey) {
            return Err(DosError::Busy);
        }
        self.mounts.insert(normalize_drive(drive)?, mount);
        Ok(())
    }

    pub fn drive(&self, drive: char) -> Option<&Mount> {
        self.mounts.get(&drive.to_ascii_uppercase())
    }

    pub fn launch(&mut self, drive: char, path: &str, args: &str) -> Result<(), DosError> {
        if matches!(self.state, RunState::Running | RunState::WaitingForKey) {
            return Err(DosError::Busy);
        }
        let drive = normalize_drive(drive)?;
        let mount = self.drive(drive).ok_or(DosError::UnmountedDrive(drive))?;
        let entry = mount.metadata(path).map_err(DosError::Mount)?;
        let bytes = mount.read(path).map_err(DosError::Mount)?;
        let path = path.replace('/', "\\");
        let cwd = path
            .rsplit_once('\\')
            .map_or("", |(parent, _)| parent)
            .trim_matches('\\');
        let name = if cwd.is_empty() {
            format!("{drive}:\\{}", entry.name)
        } else {
            format!("{drive}:\\{cwd}\\{}", entry.name)
        };
        let mut cpu = loader::load(&bytes, &name, args)?;
        let mut video = Adapter::new();
        video::initialize(&mut cpu, &mut video);
        video.copy_console(&self.video, &self.cpu, &mut cpu);
        self.cpu = cpu;
        self.video = video;
        self.files = DosFiles::new(drive, cwd.to_owned());
        self.pending = None;
        self.keys.clear();
        self.scancodes.clear();
        self.modifiers = 0;
        self.locks = 0;
        self.output.clear();
        self.dta = (PSP, 0x80);
        self.blocks = BTreeMap::from([(PSP, CONVENTIONAL_END - PSP)]);
        self.line.clear();
        self.state = RunState::Running;
        Ok(())
    }

    pub fn key(&mut self, ascii: u8, scan: u8) {
        if !self.update_modifier(scan, true)
            && (ascii != 0 || scan != 0)
            && self.keys.len() < MAX_KEYS
        {
            self.keys.push_back((ascii, scan));
        }
        self.raw_key(scan);
    }

    pub fn key_up(&mut self, scan: u8) {
        if scan != 0 {
            self.update_modifier(scan, false);
            self.raw_key(scan | 0x80);
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

    fn raw_key(&mut self, scan: u8) {
        if scan != 0 && self.scancodes.len() < MAX_KEYS {
            self.scancodes.push_back(scan);
        }
    }

    pub fn video(&self) -> Video<'_> {
        self.video.frame(&self.cpu)
    }

    pub fn write_console(&mut self, bytes: &[u8]) {
        self.video.write(&mut self.cpu, bytes);
    }

    pub fn clear_console(&mut self) {
        self.video.clear(&mut self.cpu);
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    pub fn stop(&mut self) {
        if matches!(self.state, RunState::Running | RunState::WaitingForKey) {
            self.exit(3);
        }
    }

    pub fn run(&mut self, budget: usize) -> Result<RunState, DosError> {
        if matches!(self.state, RunState::Idle | RunState::Exited(_)) {
            return Ok(self.state);
        }
        self.state = RunState::Running;
        self.update_clock();
        if let Some(pending) = self.pending.take() {
            self.resume(pending);
        }
        for _ in 0..budget {
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

    fn step(&mut self) -> Result<(), DosError> {
        match self.cpu.step().map_err(DosError::Cpu)? {
            Step::Continue => {}
            Step::Interrupt(vector) => self.interrupt(vector)?,
            Step::PortRead { port, width } => {
                let mut value = 0;
                for byte in 0..width {
                    let port = port.wrapping_add(u16::from(byte));
                    let part = match port {
                        0x60 => self.scancodes.pop_front().unwrap_or(0),
                        0x64 => u8::from(!self.scancodes.is_empty()),
                        _ => self.video.port_read(port).unwrap_or(0xff),
                    };
                    value |= u32::from(part) << (u32::from(byte) * 8);
                }
                self.cpu.complete_port_read(width, value);
            }
            Step::PortWrite { port, width, value } => {
                for byte in 0..width {
                    self.video.port_write(
                        port.wrapping_add(u16::from(byte)),
                        (value >> (byte * 8)) as u8,
                    );
                }
            }
            Step::Halt => self.exit(0),
        }
        Ok(())
    }

    fn update_clock(&mut self) -> u32 {
        let ticks =
            ((self.clock.elapsed().as_micros() * 1_193_182 / 65_536_000_000) % 0x1800b0) as u32;
        self.cpu.write_u16(0x40, 0x6c, ticks as u16);
        self.cpu.write_u16(0x40, 0x6e, (ticks >> 16) as u16);
        ticks
    }

    fn exit(&mut self, code: u8) {
        self.state = RunState::Exited(code);
        self.pending = None;
        self.files.close_all();
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

fn normalize_drive(drive: char) -> Result<char, DosError> {
    if drive.is_ascii_alphabetic() {
        Ok(drive.to_ascii_uppercase())
    } else {
        Err(DosError::InvalidDrive(drive))
    }
}
