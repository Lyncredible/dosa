//! A headless DOS process runtime with a BIOS text and VGA display.

mod bios;
mod files;
mod keyboard;
mod loader;
mod mouse;
mod pic;
mod pit;
mod services;
mod speaker;
mod vga;
mod video;

#[cfg(test)]
mod tests;

pub use mouse::MouseCursor;
pub use video::{TextFrame, Video};

use bios::{Hook, TIMER_VECTOR};
pub use dosa_cpu::CpuProfile;
use dosa_cpu::{Cpu, CpuError, Step};
use dosa_mount::{Mount, MountError};
use files::DosFiles;
use keyboard::{KEYBOARD_VECTOR, Keyboard, MAX_KEYS};
use loader::{CONVENTIONAL_END, PSP};
use mouse::Mouse;
use pic::Pic;
use pit::Pit;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::time::Instant;
use video::Adapter;

const CARRY: u16 = 1;
const ZERO: u16 = 0x40;
const TIMER_SAMPLE_INTERVAL: usize = 256;

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

#[derive(Clone, Copy)]
enum Pending {
    Bios,
    Console { echo: bool },
    Line,
    HandleRead,
}

#[derive(Clone, Copy)]
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

pub struct Machine {
    cpu: Cpu,
    cpu_profile: CpuProfile,
    video: Adapter,
    mounts: BTreeMap<char, Mount>,
    files: DosFiles,
    state: RunState,
    pending: Vec<PendingCall>,
    keys: VecDeque<(u8, u8)>,
    keyboard: Keyboard,
    mouse: Mouse,
    pic: Pic,
    pit: Pit,
    speaker: speaker::Speaker,
    modifiers: u8,
    locks: u8,
    output: Vec<u8>,
    clock: Instant,
    paused_since: Option<Instant>,
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
        bios::install(&mut cpu, 0);
        Mouse::install(&mut cpu);
        Self {
            cpu,
            cpu_profile: CpuProfile::default(),
            video,
            mounts: BTreeMap::new(),
            files: DosFiles::new('C', String::new()),
            state: RunState::Idle,
            pending: Vec::new(),
            keys: VecDeque::new(),
            keyboard: Keyboard::default(),
            mouse: Mouse::default(),
            pic: Pic::default(),
            pit: Pit::new(0),
            speaker: speaker::Speaker::new(0),
            modifiers: 0,
            locks: 0,
            output: Vec::new(),
            clock: Instant::now(),
            paused_since: None,
            dta: (PSP, 0x80),
            blocks: BTreeMap::new(),
            line: Vec::new(),
        }
    }

    pub fn mount(&mut self, drive: char, mount: Mount) -> Result<(), DosError> {
        if self.state.is_active() {
            return Err(DosError::Busy);
        }
        self.mounts.insert(normalize_drive(drive)?, mount);
        Ok(())
    }

    pub fn drive(&self, drive: char) -> Option<&Mount> {
        self.mounts.get(&drive.to_ascii_uppercase())
    }

    pub fn set_cpu_profile(&mut self, profile: CpuProfile) -> Result<(), DosError> {
        if self.state.is_active() {
            return Err(DosError::Busy);
        }
        self.cpu_profile = profile;
        Ok(())
    }

    pub fn set_paused(&mut self, paused: bool) {
        if paused && self.state.is_active() {
            self.paused_since.get_or_insert_with(Instant::now);
        } else if !paused && let Some(since) = self.paused_since.take() {
            self.clock += since.elapsed();
        }
    }

    pub fn launch(&mut self, drive: char, path: &str, args: &str) -> Result<(), DosError> {
        if self.state.is_active() {
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
        let mut cpu = loader::load(&bytes, &name, args, self.cpu_profile)?;
        let mut video = Adapter::new();
        video::initialize(&mut cpu, &mut video);
        let now = pit::ticks(self.clock.elapsed());
        bios::install(&mut cpu, now);
        Mouse::install(&mut cpu);
        video.copy_console(&self.video, &self.cpu, &mut cpu);
        self.cpu = cpu;
        self.video = video;
        self.files = DosFiles::new(drive, cwd.to_owned());
        self.pending.clear();
        self.keys.clear();
        self.keyboard = Keyboard::default();
        self.mouse = Mouse::default();
        self.pic = Pic::default();
        self.pit = Pit::new(now);
        self.speaker = speaker::Speaker::new(now);
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
        self.video.write(&mut self.cpu, bytes);
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

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    /// Drain mono PC audio at 48 kHz. The queue retains at most 200 milliseconds.
    pub fn take_audio(&mut self) -> Vec<f32> {
        self.speaker.drain()
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
        self.state = RunState::Running;
        let now = pit::ticks(self.clock.elapsed());
        self.pit.advance(now);
        self.speaker.advance(now);
        for instruction in 0..budget {
            if !matches!(self.state, RunState::Running | RunState::WaitingForKey) {
                break;
            }
            if instruction != 0 && instruction % TIMER_SAMPLE_INTERVAL == 0 {
                let now = pit::ticks(self.clock.elapsed());
                self.pit.advance(now);
                self.speaker.advance(now);
            }
            self.poll_keyboard_wait();
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

    fn poll_keyboard_wait(&mut self) {
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
        let interrupts_enabled = self.cpu.can_accept_maskable_interrupt() && !self.cpu.is_halted();
        self.mouse
            .dispatch_callback(&mut self.cpu, interrupts_enabled);
        self.service_hardware_interrupt();
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
        match self.cpu.step().map_err(DosError::Cpu)? {
            Step::Continue => {}
            Step::Interrupt(vector) => self.interrupt(vector)?,
            Step::PortRead { port, width } => {
                self.speaker.advance(pit::ticks(self.clock.elapsed()));
                let mut value = 0;
                for byte in 0..width {
                    let port = port.wrapping_add(u16::from(byte));
                    let requests = self.irq_requests();
                    let part = self
                        .pic
                        .port_read(port, requests)
                        .or_else(|| self.speaker.read_port(port))
                        .or_else(|| self.pit.port_read(port))
                        .or_else(|| self.keyboard.port_read(port))
                        .or_else(|| self.video.port_read(port))
                        .unwrap_or(0xff);
                    value |= u32::from(part) << (u32::from(byte) * 8);
                }
                self.cpu.complete_port_read(width, value);
            }
            Step::PortWrite { port, width, value } => {
                for byte in 0..width {
                    let port = port.wrapping_add(u16::from(byte));
                    let value = (value >> (byte * 8)) as u8;
                    self.speaker.advance(pit::ticks(self.clock.elapsed()));
                    if !self.speaker.write_port(port, value)
                        && !self.pic.port_write(port, value)
                        && !self.pit.port_write(port, value)
                        && !self.keyboard.port_write(port, value)
                    {
                        self.video.port_write(port, value);
                    }
                }
            }
            Step::Halt => self.state = RunState::WaitingForInterrupt,
        }
        Ok(())
    }

    fn irq_requests(&self) -> u8 {
        u8::from(self.pit.irq_requested()) | (u8::from(self.keyboard.irq_requested()) << 1)
    }

    fn service_hardware_interrupt(&mut self) -> bool {
        if !self.cpu.can_accept_maskable_interrupt() {
            return false;
        }
        let Some(irq) = self.pic.next_irq(self.irq_requests()) else {
            return false;
        };
        let vector = TIMER_VECTOR + irq;
        if irq == 0 {
            self.pit.acknowledge_irq();
        }
        if self.has_interrupt_handler(vector) {
            if vector == KEYBOARD_VECTOR {
                self.keyboard.acknowledge_irq();
            }
            self.pic.acknowledge(irq);
            self.cpu.enter_interrupt(vector);
            return true;
        }
        false
    }

    fn exit(&mut self, code: u8) {
        self.set_paused(false);
        self.state = RunState::Exited(code);
        self.pending.clear();
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
