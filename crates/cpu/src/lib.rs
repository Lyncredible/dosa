//! x86 execution, with software interrupts and I/O delegated to the machine.
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use std::fmt;

use control::{Control, Segment};

mod arithmetic;
mod control;
mod execute;
mod profile;

pub use profile::CpuProfile;

const A20_LINE: usize = 1 << 20;
const EXTENDED_MACHINE_RAM: usize = 16 << 20;
const CF: u16 = 1;
const PF: u16 = 1 << 2;
const AF: u16 = 1 << 4;
const ZF: u16 = 1 << 6;
const SF: u16 = 1 << 7;
const TF: u16 = 1 << 8;
const IF: u16 = 1 << 9;
const DF: u16 = 1 << 10;
const OF: u16 = 1 << 11;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Registers {
    pub ax: u16,
    pub bx: u16,
    pub cx: u16,
    pub dx: u16,
    pub sp: u16,
    pub bp: u16,
    pub si: u16,
    pub di: u16,
    pub ip: u16,
    pub cs: u16,
    pub ds: u16,
    pub es: u16,
    pub ss: u16,
    pub flags: u16,
}

impl Default for Registers {
    fn default() -> Self {
        Self {
            ax: 0,
            bx: 0,
            cx: 0,
            dx: 0,
            sp: 0,
            bp: 0,
            si: 0,
            di: 0,
            ip: 0,
            cs: 0,
            ds: 0,
            es: 0,
            ss: 0,
            flags: 2,
        }
    }
}

/// The complete guest register state saved across a host-serviced callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisterSnapshot {
    registers: Registers,
    ip_high: u16,
    segments: [Option<Segment>; 6],
    high: [u16; 8],
    fs: u16,
    gs: u16,
    interrupt_shadow: bool,
    halted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Continue,
    /// IP points to the next instruction. The machine may service the call
    /// directly or dispatch it through the guest's interrupt vector table.
    Interrupt(u8),
    PortRead {
        port: u16,
        width: u8,
    },
    PortWrite {
        port: u16,
        width: u8,
        value: u32,
    },
    Halt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuErrorKind {
    Unsupported,
    UnavailableOnProfile(CpuProfile),
    InstructionFetch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuError {
    pub cs: u16,
    pub ip: u32,
    pub mnemonic: Mnemonic,
    pub kind: CpuErrorKind,
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CpuErrorKind::Unsupported => {
                write!(f, "unsupported {:?} instruction or operand", self.mnemonic)?;
            }
            CpuErrorKind::InstructionFetch => {
                f.write_str("invalid protected-mode instruction fetch")?
            }
            CpuErrorKind::UnavailableOnProfile(profile) => {
                write!(
                    f,
                    "{:?} encoding unavailable on the {profile} CPU profile",
                    self.mnemonic
                )?;
            }
        }
        write!(f, " at {:04X}:{:04X}", self.cs, self.ip)
    }
}

impl std::error::Error for CpuError {}

/// A byte-addressed device in the guest's physical memory space.
///
/// Offsets are relative to the mapping base. Multi-byte guest accesses reach
/// the device in ascending address order, one byte at a time.
/// Instruction fetch can read ahead up to the 15-byte decoder window.
pub trait MemoryDevice: Send + Sync {
    fn read(&self, offset: usize) -> u8;
    fn write(&self, offset: usize, value: u8);
    /// Inspect a byte without changing the device, if supported.
    fn peek(&self, _offset: usize) -> Option<u8> {
        None
    }
}

struct MemoryMapping {
    start: usize,
    end: usize,
    device: Box<dyn MemoryDevice>,
}

pub struct Cpu {
    pub regs: Registers,
    /// Physical RAM. Direct accesses bypass the A20 gate and mapped devices.
    pub memory: Box<[u8]>,
    mapping: Option<MemoryMapping>,
    profile: CpuProfile,
    control: Control,
    address_mask: u32,
    ip_high: u16,
    high: [u16; 8],
    fs: u16,
    gs: u16,
    interrupt_shadow: bool,
    halted: bool,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn save_registers(&self) -> RegisterSnapshot {
        RegisterSnapshot {
            registers: self.regs,
            ip_high: self.ip_high,
            segments: self.control.segments,
            high: self.high,
            fs: self.fs,
            gs: self.gs,
            interrupt_shadow: self.interrupt_shadow,
            halted: self.halted,
        }
    }

    pub fn restore_registers(&mut self, snapshot: RegisterSnapshot) {
        self.regs = snapshot.registers;
        self.ip_high = snapshot.ip_high;
        self.control.segments = snapshot.segments;
        self.high = snapshot.high;
        self.fs = snapshot.fs;
        self.gs = snapshot.gs;
        self.interrupt_shadow = snapshot.interrupt_shadow;
        self.halted = snapshot.halted;
    }

    pub fn new() -> Self {
        Self::with_profile(CpuProfile::default())
    }

    pub fn with_profile(profile: CpuProfile) -> Self {
        Self {
            regs: Registers {
                flags: profile.normalize_flags(0),
                ..Registers::default()
            },
            memory: vec![
                0;
                if profile == CpuProfile::I8086 {
                    A20_LINE
                } else {
                    EXTENDED_MACHINE_RAM
                }
            ]
            .into_boxed_slice(),
            mapping: None,
            profile,
            control: Control::new(profile),
            address_mask: Self::bus_mask(profile) & !(A20_LINE as u32),
            ip_high: 0,
            high: [0; 8],
            fs: 0,
            gs: 0,
            interrupt_shadow: false,
            halted: false,
        }
    }

    pub fn profile(&self) -> CpuProfile {
        self.profile
    }

    /// Whether a maskable interrupt can be accepted at this instruction boundary.
    pub fn can_accept_maskable_interrupt(&self) -> bool {
        self.flag(IF) && !self.interrupt_shadow
    }

    /// A halted processor will resume at the next instruction after interrupt return.
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// Whether the machine accepts the A20 gate input.
    pub fn has_a20(&self) -> bool {
        self.profile != CpuProfile::I8086
    }

    pub fn a20_enabled(&self) -> bool {
        self.address_mask & A20_LINE as u32 != 0
    }

    pub fn extended_memory_kib(&self) -> u16 {
        (self.memory.len().saturating_sub(A20_LINE) / 1024) as u16
    }

    pub fn set_a20(&mut self, enabled: bool) {
        let gate = if enabled {
            u32::MAX
        } else {
            !(A20_LINE as u32)
        };
        self.address_mask = Self::bus_mask(self.profile) & gate;
    }

    /// Install or replace a device mapping within the installed physical address
    /// space. An empty or out-of-range mapping leaves the current mapping in
    /// place and returns false.
    pub fn map_memory(&mut self, base: usize, len: usize, device: Box<dyn MemoryDevice>) -> bool {
        let Some(end) = base
            .checked_add(len)
            .filter(|&end| len != 0 && end <= self.memory.len())
        else {
            return false;
        };
        self.mapping = Some(MemoryMapping {
            start: base,
            end,
            device,
        });
        true
    }

    pub fn unmap_memory(&mut self) {
        self.mapping = None;
    }

    pub fn read_u8(&self, segment: u16, offset: u16) -> u8 {
        self.read_byte(Self::linear(segment, offset.into()))
    }

    pub fn read_u16(&self, segment: u16, offset: u16) -> u16 {
        self.read_mem(Self::linear(segment, offset.into()), 2) as u16
    }

    pub fn write_u8(&mut self, segment: u16, offset: u16, value: u8) {
        self.write_byte(Self::linear(segment, offset.into()), value);
    }

    pub fn write_u16(&mut self, segment: u16, offset: u16, value: u16) {
        self.write_mem(Self::linear(segment, offset.into()), 2, value.into());
    }

    /// Copy contiguous bytes through the guest bus using the current A20 state.
    pub fn load(&mut self, segment: u16, offset: u16, bytes: &[u8]) {
        let address = Self::linear(segment, offset.into());
        for (index, &byte) in bytes.iter().enumerate() {
            self.write_byte(address.wrapping_add(index), byte);
        }
    }

    /// Full instruction pointer, including the high word of a 386 code offset.
    pub fn instruction_pointer(&self) -> u32 {
        u32::from(self.ip_high) << 16 | u32::from(self.regs.ip)
    }

    fn set_ip(&mut self, ip: u32) {
        self.regs.ip = ip as u16;
        self.ip_high = (ip >> 16) as u16;
    }

    pub fn step(&mut self) -> Result<Step, CpuError> {
        if self.halted {
            return Ok(Step::Halt);
        }
        let cs = self.regs.cs;
        let ip = self.instruction_pointer();
        let code_segment = self.control.segments[Register::CS.number()];
        let bitness = if code_segment.is_some_and(|segment| segment.wide) {
            32
        } else {
            16
        };
        let fetch_error = CpuError {
            cs,
            ip,
            mnemonic: Mnemonic::INVALID,
            kind: CpuErrorKind::InstructionFetch,
        };
        let linear = self.code_address(ip, 1).ok_or(fetch_error)?;
        let address = self.physical(linear);
        let end = address
            .checked_add(15)
            .filter(|&end| end <= self.memory.len());
        let contiguous = end.is_some_and(|end| self.physical(linear.wrapping_add(14)) == end - 1);
        let unwrapped_ip = code_segment.is_some() || ip <= 0x10000 - 15;
        let ram_only = self
            .mapping
            .as_ref()
            .is_none_or(|map| address >= map.end || end.is_some_and(|end| end <= map.start));
        let tail: [u8; 15];
        let readable_window = self.code_address(ip, 15).is_some();
        let code = if let Some(end) =
            end.filter(|_| unwrapped_ip && ram_only && readable_window && contiguous)
        {
            &self.memory[address..end]
        } else {
            tail = std::array::from_fn(|index| {
                let offset = ip.wrapping_add(index as u32);
                let offset = if code_segment.is_none() {
                    offset & 0xffff
                } else {
                    offset
                };
                self.code_address(offset, 1)
                    .map_or(0, |address| self.read_byte(address))
            });
            &tail
        };
        // Prior to SSE2, the PAUSE encoding was a REP NOP.
        let instruction =
            Decoder::with_ip(bitness, code, u64::from(ip), DecoderOptions::NO_PAUSE).decode();
        if code_segment.is_some() && self.code_address(ip, instruction.len()).is_none() {
            return Err(fetch_error);
        }
        if !instruction.is_invalid()
            && !self
                .profile
                .allows(&instruction, &code[..instruction.len()])
        {
            return Err(CpuError {
                cs,
                ip,
                mnemonic: instruction.mnemonic(),
                kind: CpuErrorKind::UnavailableOnProfile(self.profile),
            });
        }
        // STI delays IRQs only when it sets IF. A stack-segment load also
        // suppresses IRQs until the instruction that follows it has executed.
        let new_shadow = !self.interrupt_shadow
            && match instruction.mnemonic() {
                Mnemonic::Sti => !self.flag(IF),
                Mnemonic::Mov | Mnemonic::Pop => {
                    instruction.op0_kind() == OpKind::Register
                        && instruction.op0_register() == Register::SS
                }
                _ => false,
            };
        let next = if code_segment.is_some() {
            instruction.next_ip32()
        } else {
            u32::from(instruction.next_ip16())
        };
        self.set_ip(next);
        let step = self.execute(&instruction, ip).ok_or_else(|| {
            self.set_ip(ip);
            CpuError {
                cs,
                ip,
                mnemonic: instruction.mnemonic(),
                kind: CpuErrorKind::Unsupported,
            }
        })?;
        self.interrupt_shadow = new_shadow;
        self.halted = step == Step::Halt;
        Ok(step)
    }

    pub fn enter_interrupt(&mut self, vector: u8) -> Option<()> {
        let (target_ip, target_cs, descriptor, width, disable_irq) = if self.protected_mode() {
            let gate = self.interrupt_gate(vector)?;
            (
                gate.offset,
                gate.selector,
                Some(gate.code),
                gate.width,
                gate.disable_irq,
            )
        } else {
            let offset = u16::from(vector) * 4;
            (
                u32::from(self.read_u16(0, offset)),
                self.read_u16(0, offset + 2),
                None,
                2,
                true,
            )
        };
        self.check_stack_push(width, 3)?;
        self.push(
            width,
            u32::from(self.profile.normalize_flags(self.regs.flags)),
        )?;
        self.push(width, u32::from(self.regs.cs))?;
        self.push(width, self.instruction_pointer())?;
        self.regs.flags &= !(TF | if disable_irq { IF } else { 0 });
        self.interrupt_shadow = false;
        self.halted = false;
        self.regs.cs = target_cs;
        self.control.segments[Register::CS.number()] = descriptor;
        self.set_ip(target_ip);
        Some(())
    }

    pub fn complete_port_read(&mut self, width: u8, value: u32) {
        if let Some(register) = Self::accumulator(width) {
            self.set_reg(register, value);
        }
    }

    fn accumulator(width: u8) -> Option<Register> {
        match width {
            1 => Some(Register::AL),
            2 => Some(Register::AX),
            4 => Some(Register::EAX),
            _ => None,
        }
    }

    fn linear(segment: u16, offset: u32) -> usize {
        (u32::from(segment) << 4).wrapping_add(offset) as usize
    }

    fn bus_mask(profile: CpuProfile) -> u32 {
        match profile {
            CpuProfile::I8086 => (A20_LINE - 1) as u32,
            CpuProfile::I286 => (EXTENDED_MACHINE_RAM - 1) as u32,
            CpuProfile::I386 => u32::MAX,
        }
    }

    fn physical(&self, linear: usize) -> usize {
        (linear as u32 & self.address_mask) as usize
    }

    fn read_byte(&self, address: usize) -> u8 {
        let address = self.physical(address);
        if let Some(map) = &self.mapping
            && (map.start..map.end).contains(&address)
        {
            map.device.read(address - map.start)
        } else {
            self.memory.get(address).copied().unwrap_or(0xff)
        }
    }

    /// Inspect the current code stream using the cached code segment, when loaded.
    pub fn peek_code_byte(&self, relative: u8) -> Option<u8> {
        let mut offset = self.instruction_pointer().wrapping_add(u32::from(relative));
        if self.control.segments[Register::CS.number()].is_none() {
            offset &= 0xffff;
        }
        self.peek_byte(self.code_address(offset, 1)?)
    }

    /// Inspect the stack without changing SP, ESP or mapped device state.
    pub fn peek_stack_byte(&self, relative: u8) -> Option<u8> {
        self.peek_byte(self.segment_address(
            Register::SS,
            self.stack_offset(relative.into()),
            1,
            false,
        )?)
    }

    /// The source index in the active code segment's default address size.
    pub fn source_pointer(&self) -> u32 {
        if self.control.segments[Register::CS.number()].is_some_and(|segment| segment.wide) {
            u32::from(self.high[6]) << 16 | u32::from(self.regs.si)
        } else {
            u32::from(self.regs.si)
        }
    }

    pub fn peek_data_byte(&self, relative: u8) -> Option<u8> {
        let mut offset = self.source_pointer().wrapping_add(u32::from(relative));
        if self.control.segments[Register::DS.number()].is_none() {
            offset &= 0xffff;
        }
        self.peek_byte(self.segment_address(Register::DS, offset, 1, false)?)
    }

    pub fn peek_u8(&self, segment: u16, offset: u16) -> Option<u8> {
        self.peek_byte(Self::linear(segment, offset.into()))
    }

    fn peek_byte(&self, address: usize) -> Option<u8> {
        let address = self.physical(address);
        if let Some(map) = &self.mapping
            && (map.start..map.end).contains(&address)
        {
            map.device.peek(address - map.start)
        } else {
            self.memory.get(address).copied()
        }
    }

    fn write_byte(&mut self, address: usize, value: u8) {
        let address = self.physical(address);
        if let Some(map) = &self.mapping
            && (map.start..map.end).contains(&address)
        {
            map.device.write(address - map.start, value);
        } else if let Some(byte) = self.memory.get_mut(address) {
            *byte = value;
        }
    }

    fn read_mem(&self, address: usize, width: u8) -> u32 {
        let mut bytes = [0; 4];
        for (index, byte) in bytes[..usize::from(width)].iter_mut().enumerate() {
            *byte = self.read_byte(address.wrapping_add(index));
        }
        u32::from_le_bytes(bytes)
    }

    fn write_mem(&mut self, address: usize, width: u8, value: u32) {
        for (index, &byte) in value.to_le_bytes()[..usize::from(width)].iter().enumerate() {
            self.write_byte(address.wrapping_add(index), byte);
        }
    }

    fn offset(&self, ins: &Instruction) -> Option<u32> {
        let reg = |r| {
            if r == Register::None {
                Some(0)
            } else {
                self.get_reg(r)
            }
        };
        let offset = reg(ins.memory_base())?
            .wrapping_add(reg(ins.memory_index())?.wrapping_mul(ins.memory_index_scale()))
            .wrapping_add(ins.memory_displacement32());
        let address32 = ins.memory_base().size() == 4
            || ins.memory_index().size() == 4
            || ins.memory_displ_size() == 4;
        Some(if address32 { offset } else { offset & 0xffff })
    }

    fn memory_address(&self, ins: &Instruction, write: bool) -> Option<usize> {
        self.segment_address(
            ins.memory_segment(),
            self.offset(ins)?,
            ins.memory_size().size(),
            write,
        )
    }

    fn code_address(&self, offset: u32, width: usize) -> Option<usize> {
        if let Some(segment) = self.control.segments[Register::CS.number()] {
            segment.address(true, offset, width, false)
        } else {
            Some(Self::linear(self.regs.cs, offset))
        }
    }

    fn segment_address(
        &self,
        register: Register,
        offset: u32,
        width: usize,
        write: bool,
    ) -> Option<usize> {
        if let Some(segment) = self.control.segments[register.number()] {
            segment.address(false, offset, width, write)
        } else {
            Some(Self::linear(self.get_reg(register)? as u16, offset))
        }
    }

    fn width(ins: &Instruction, index: u32) -> Option<u8> {
        let width = match ins.op_kind(index) {
            OpKind::Register => ins.op_register(index).size(),
            OpKind::Memory => ins.memory_size().size(),
            OpKind::Immediate8 | OpKind::Immediate8_2nd => 1,
            OpKind::Immediate16 | OpKind::Immediate8to16 => 2,
            OpKind::Immediate32 | OpKind::Immediate8to32 => 4,
            _ => return None,
        };
        matches!(width, 1 | 2 | 4).then_some(width as u8)
    }

    fn read_op(&self, ins: &Instruction, index: u32) -> Option<u32> {
        Some(match ins.op_kind(index) {
            OpKind::Register => self.get_reg(ins.op_register(index))?,
            OpKind::Memory => {
                self.read_mem(self.memory_address(ins, false)?, Self::width(ins, index)?)
            }
            OpKind::Immediate8 => ins.immediate8().into(),
            OpKind::Immediate8_2nd => ins.immediate8_2nd().into(),
            OpKind::Immediate16 => ins.immediate16().into(),
            OpKind::Immediate32 => ins.immediate32(),
            OpKind::Immediate8to16 => i32::from(ins.immediate8to16()) as u32,
            OpKind::Immediate8to32 => ins.immediate8to32() as u32,
            _ => return None,
        })
    }

    fn write_op(&mut self, ins: &Instruction, index: u32, value: u32) -> Option<()> {
        match ins.op_kind(index) {
            OpKind::Register => self.set_reg(ins.op_register(index), value)?,
            OpKind::Memory => self.write_mem(
                self.memory_address(ins, true)?,
                Self::width(ins, index)?,
                value,
            ),
            _ => return None,
        }
        Some(())
    }

    fn gpr(&mut self, index: usize) -> &mut u16 {
        match index {
            0 => &mut self.regs.ax,
            1 => &mut self.regs.cx,
            2 => &mut self.regs.dx,
            3 => &mut self.regs.bx,
            4 => &mut self.regs.sp,
            5 => &mut self.regs.bp,
            6 => &mut self.regs.si,
            7 => &mut self.regs.di,
            _ => unreachable!("only the eight legacy general registers are supported"),
        }
    }

    fn register_slice(register: Register) -> Option<(usize, u32, u32)> {
        use Register::*;
        Some(match register {
            AL | CL | DL | BL => (register.number(), 0, 0xff),
            AH | CH | DH | BH => (register.number() - 4, 8, 0xff),
            AX | CX | DX | BX | SP | BP | SI | DI => (register.number(), 0, 0xffff),
            EAX | ECX | EDX | EBX | ESP | EBP | ESI | EDI => (register.number(), 0, u32::MAX),
            _ => return Option::None,
        })
    }

    fn get_reg(&self, register: Register) -> Option<u32> {
        use Register::*;
        Some(match register {
            ES => self.regs.es.into(),
            CS => self.regs.cs.into(),
            SS => self.regs.ss.into(),
            DS => self.regs.ds.into(),
            FS => self.fs.into(),
            GS => self.gs.into(),
            CR0 if self.profile == CpuProfile::I386 => self.control.cr0,
            _ => {
                let (index, shift, mask) = Self::register_slice(register)?;
                let lows = [
                    self.regs.ax,
                    self.regs.cx,
                    self.regs.dx,
                    self.regs.bx,
                    self.regs.sp,
                    self.regs.bp,
                    self.regs.si,
                    self.regs.di,
                ];
                ((u32::from(lows[index]) | u32::from(self.high[index]) << 16) >> shift) & mask
            }
        })
    }

    fn set_reg(&mut self, register: Register, value: u32) -> Option<()> {
        use Register::*;
        if register.is_segment_register() {
            self.control.segments[register.number()] = self.descriptor(register, value as u16)?;
        }
        match register {
            ES => self.regs.es = value as u16,
            CS => self.regs.cs = value as u16,
            SS => self.regs.ss = value as u16,
            DS => self.regs.ds = value as u16,
            FS => self.fs = value as u16,
            GS => self.gs = value as u16,
            CR0 if self.profile == CpuProfile::I386 => self.write_cr0(value)?,
            _ => {
                let (index, shift, mask) = Self::register_slice(register)?;
                let low = self.gpr(index);
                let low_mask = (mask << shift) as u16;
                *low = (*low & !low_mask) | (((value & mask) << shift) as u16);
                if mask == u32::MAX {
                    self.high[index] = (value >> 16) as u16;
                }
            }
        }
        Some(())
    }

    fn wide_stack(&self) -> bool {
        self.control.segments[Register::SS.number()].is_some_and(|segment| segment.wide)
    }

    /// Address of the top of stack using the active stack segment width.
    pub fn stack_pointer(&self) -> u32 {
        let high = if self.wide_stack() {
            u32::from(self.high[4]) << 16
        } else {
            0
        };
        high | u32::from(self.regs.sp)
    }

    fn set_stack_pointer(&mut self, pointer: u32) {
        if self.wide_stack() {
            self.high[4] = (pointer >> 16) as u16;
        }
        self.regs.sp = pointer as u16;
    }

    fn stack_offset(&self, change: u32) -> u32 {
        let pointer = self.stack_pointer().wrapping_add(change);
        if self.wide_stack() {
            pointer
        } else {
            pointer & 0xffff
        }
    }

    fn adjust_stack(&mut self, change: u32) {
        self.set_stack_pointer(self.stack_offset(change));
    }

    fn check_stack_push(&self, width: u8, count: u8) -> Option<()> {
        if self.control.segments[Register::SS.number()].is_none() {
            return Some(());
        }
        for word in 1..=count {
            let offset = self.stack_offset(0u32.wrapping_sub(u32::from(word) * u32::from(width)));
            self.segment_address(Register::SS, offset, usize::from(width), true)?;
        }
        Some(())
    }

    fn push(&mut self, width: u8, value: u32) -> Option<()> {
        let offset = self.stack_offset(0u32.wrapping_sub(width.into()));
        let address = self.segment_address(Register::SS, offset, usize::from(width), true)?;
        self.write_mem(address, width, value);
        self.set_stack_pointer(offset);
        Some(())
    }

    fn peek_stack(&self, width: u8, relative: u32) -> Option<u32> {
        let offset = self.stack_offset(relative);
        let address = self.segment_address(Register::SS, offset, usize::from(width), false)?;
        Some(self.read_mem(address, width))
    }

    fn pop(&mut self, width: u8) -> Option<u32> {
        let value = self.peek_stack(width, 0)?;
        self.adjust_stack(width.into());
        Some(value)
    }

    fn flag(&self, mask: u16) -> bool {
        self.regs.flags & mask != 0
    }

    fn set_flag(&mut self, mask: u16, set: bool) {
        if set {
            self.regs.flags |= mask;
        } else {
            self.regs.flags &= !mask;
        }
    }
}

#[cfg(test)]
mod tests;
