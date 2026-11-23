//! Real-mode x86 execution, with software interrupts and I/O delegated to the machine.
use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use std::fmt;

mod arithmetic;
mod execute;
mod profile;

pub use profile::CpuProfile;

const MEMORY_SIZE: usize = 1 << 20;
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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuError {
    pub cs: u16,
    pub ip: u16,
    pub mnemonic: Mnemonic,
    pub kind: CpuErrorKind,
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            CpuErrorKind::Unsupported => {
                write!(f, "unsupported {:?} instruction or operand", self.mnemonic)?;
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
}

struct MemoryMapping {
    start: usize,
    end: usize,
    device: Box<dyn MemoryDevice>,
}

pub struct Cpu {
    pub regs: Registers,
    /// Backing RAM with A20 disabled. Direct accesses bypass mapped devices.
    pub memory: Box<[u8]>,
    mapping: Option<MemoryMapping>,
    profile: CpuProfile,
    high: [u16; 8],
    fs: u16,
    gs: u16,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    pub fn new() -> Self {
        Self::with_profile(CpuProfile::default())
    }

    pub fn with_profile(profile: CpuProfile) -> Self {
        Self {
            regs: Registers {
                flags: profile.normalize_flags(0),
                ..Registers::default()
            },
            memory: vec![0; MEMORY_SIZE].into_boxed_slice(),
            mapping: None,
            profile,
            high: [0; 8],
            fs: 0,
            gs: 0,
        }
    }

    pub fn profile(&self) -> CpuProfile {
        self.profile
    }

    /// Install or replace a device mapping within the 1 MiB physical address
    /// space. An empty or out-of-range mapping leaves the current mapping in
    /// place and returns false.
    pub fn map_memory(&mut self, base: usize, len: usize, device: Box<dyn MemoryDevice>) -> bool {
        let Some(end) = base
            .checked_add(len)
            .filter(|&end| len != 0 && end <= MEMORY_SIZE)
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

    /// Copy contiguous bytes through the guest bus with 20-bit wraparound.
    pub fn load(&mut self, segment: u16, offset: u16, bytes: &[u8]) {
        let address = Self::linear(segment, offset.into());
        for (index, &byte) in bytes.iter().enumerate() {
            self.write_byte(address.wrapping_add(index) & (MEMORY_SIZE - 1), byte);
        }
    }

    pub fn step(&mut self) -> Result<Step, CpuError> {
        let cs = self.regs.cs;
        let ip = self.regs.ip;
        let address = Self::linear(cs, ip.into());
        let unwrapped_ip = usize::from(ip) + 15 <= 1 << 16;
        let ram_only = self
            .mapping
            .as_ref()
            .is_none_or(|map| address >= map.end || address + 15 <= map.start);
        let tail: [u8; 15];
        let code = if unwrapped_ip && ram_only && address + 15 <= MEMORY_SIZE {
            &self.memory[address..address + 15]
        } else {
            tail = std::array::from_fn(|index| self.read_u8(cs, ip.wrapping_add(index as u16)));
            &tail
        };
        // Prior to SSE2, the PAUSE encoding was a REP NOP.
        let instruction =
            Decoder::with_ip(16, code, u64::from(ip), DecoderOptions::NO_PAUSE).decode();
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
        self.regs.ip = instruction.next_ip16();
        self.execute(&instruction, ip).ok_or_else(|| {
            self.regs.ip = ip;
            CpuError {
                cs,
                ip,
                mnemonic: instruction.mnemonic(),
                kind: CpuErrorKind::Unsupported,
            }
        })
    }

    pub fn enter_interrupt(&mut self, vector: u8) {
        let offset = u16::from(vector) * 4;
        let target_ip = self.read_u16(0, offset);
        let target_cs = self.read_u16(0, offset + 2);
        self.push(2, u32::from(self.profile.normalize_flags(self.regs.flags)));
        self.push(2, u32::from(self.regs.cs));
        self.push(2, u32::from(self.regs.ip));
        self.regs.flags &= !(IF | TF);
        self.regs.cs = target_cs;
        self.regs.ip = target_ip;
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
        (((u32::from(segment) << 4).wrapping_add(offset)) as usize) & (MEMORY_SIZE - 1)
    }

    fn read_byte(&self, address: usize) -> u8 {
        if let Some(map) = &self.mapping
            && (map.start..map.end).contains(&address)
        {
            map.device.read(address - map.start)
        } else {
            self.memory[address]
        }
    }

    fn write_byte(&mut self, address: usize, value: u8) {
        if let Some(map) = &self.mapping
            && (map.start..map.end).contains(&address)
        {
            map.device.write(address - map.start, value);
        } else {
            self.memory[address] = value;
        }
    }

    fn read_mem(&self, address: usize, width: u8) -> u32 {
        let mut bytes = [0; 4];
        for (index, byte) in bytes[..usize::from(width)].iter_mut().enumerate() {
            *byte = self.read_byte((address + index) & (MEMORY_SIZE - 1));
        }
        u32::from_le_bytes(bytes)
    }

    fn write_mem(&mut self, address: usize, width: u8, value: u32) {
        for (index, &byte) in value.to_le_bytes()[..usize::from(width)].iter().enumerate() {
            self.write_byte((address + index) & (MEMORY_SIZE - 1), byte);
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

    fn memory_address(&self, ins: &Instruction) -> Option<usize> {
        Some(Self::linear(
            self.get_reg(ins.memory_segment())? as u16,
            self.offset(ins)?,
        ))
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
            OpKind::Memory => self.read_mem(self.memory_address(ins)?, Self::width(ins, index)?),
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
            OpKind::Memory => {
                self.write_mem(self.memory_address(ins)?, Self::width(ins, index)?, value)
            }
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
        match register {
            ES => self.regs.es = value as u16,
            CS => self.regs.cs = value as u16,
            SS => self.regs.ss = value as u16,
            DS => self.regs.ds = value as u16,
            FS => self.fs = value as u16,
            GS => self.gs = value as u16,
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

    fn push(&mut self, width: u8, value: u32) {
        self.regs.sp = self.regs.sp.wrapping_sub(width.into());
        self.write_mem(
            Self::linear(self.regs.ss, self.regs.sp.into()),
            width,
            value,
        );
    }

    fn pop(&mut self, width: u8) -> u32 {
        let value = self.read_mem(Self::linear(self.regs.ss, self.regs.sp.into()), width);
        self.regs.sp = self.regs.sp.wrapping_add(width.into());
        value
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
