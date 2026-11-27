use super::{Control, Cpu, CpuProfile, Registers};
use serde::{Deserialize, Serialize};
use std::fmt;

const MAGIC: &[u8; 8] = b"DOSACPU\0";
const VERSION: u16 = 1;
const VERSION_TASK_REGISTER: u16 = 2;
const HEADER_BYTES: usize = 12;
const MAX_METADATA_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotError {
    Header,
    Version(u16),
    Metadata,
    MemorySize,
    Profile,
    Mapping,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Header => f.write_str("invalid CPU snapshot header"),
            Self::Version(version) => write!(f, "unsupported CPU snapshot version {version}"),
            Self::Metadata => f.write_str("invalid CPU snapshot register state"),
            Self::MemorySize => f.write_str("invalid CPU snapshot memory length"),
            Self::Profile => f.write_str("CPU snapshot is for a different processor profile"),
            Self::Mapping => f.write_str("CPU snapshot requires a different device mapping"),
        }
    }
}

impl std::error::Error for SnapshotError {}

#[derive(Serialize, Deserialize)]
struct Metadata {
    profile: CpuProfile,
    registers: Registers,
    control: Control,
    a20: bool,
    ip_high: u16,
    high: [u16; 8],
    fs: u16,
    gs: u16,
    interrupt_shadow: bool,
    halted: bool,
    mapping: Option<(u32, u32)>,
}

impl Cpu {
    /// Encode the CPU registers, bus state, and physical RAM at an instruction boundary.
    /// The owner of a mapped device captures its state separately.
    pub fn save_snapshot(&self) -> Result<Vec<u8>, SnapshotError> {
        let metadata = Metadata {
            profile: self.profile,
            registers: self.regs,
            control: self.control,
            a20: self.a20_enabled(),
            ip_high: self.ip_high,
            high: self.high,
            fs: self.fs,
            gs: self.gs,
            interrupt_shadow: self.interrupt_shadow,
            halted: self.halted,
            mapping: self.snapshot_mapping(),
        };
        let metadata = postcard::to_allocvec(&metadata).map_err(|_| SnapshotError::Metadata)?;
        let (version, metadata) = match self.control.task_register {
            Some(task) => (
                VERSION_TASK_REGISTER,
                postcard::to_extend(&task, metadata).map_err(|_| SnapshotError::Metadata)?,
            ),
            None => (VERSION, metadata),
        };
        if metadata.len() > MAX_METADATA_BYTES {
            return Err(SnapshotError::Metadata);
        }
        let mut bytes = Vec::with_capacity(HEADER_BYTES + metadata.len() + self.memory.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&(metadata.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&metadata);
        bytes.extend_from_slice(&self.memory);
        Ok(bytes)
    }

    /// Restore a CPU snapshot after rebinding the required device mapping.
    /// Validation completes before the CPU or its devices change.
    pub fn restore_snapshot(&mut self, bytes: &[u8]) -> Result<(), SnapshotError> {
        let (header, payload) = bytes
            .split_at_checked(HEADER_BYTES)
            .filter(|(header, _)| header.starts_with(MAGIC))
            .ok_or(SnapshotError::Header)?;
        let version = u16::from_le_bytes([header[8], header[9]]);
        if !matches!(version, VERSION | VERSION_TASK_REGISTER) {
            return Err(SnapshotError::Version(version));
        }
        let length = usize::from(u16::from_le_bytes([header[10], header[11]]));
        let (metadata, ram) = payload
            .split_at_checked(length)
            .filter(|_| length <= MAX_METADATA_BYTES)
            .ok_or(SnapshotError::Metadata)?;
        let (mut state, remainder) =
            postcard::take_from_bytes::<Metadata>(metadata).map_err(|_| SnapshotError::Metadata)?;
        let remainder = if version == VERSION_TASK_REGISTER {
            let (task, trailing) =
                postcard::take_from_bytes(remainder).map_err(|_| SnapshotError::Metadata)?;
            state.control.task_register = Some(task);
            trailing
        } else {
            remainder
        };
        if state.profile != self.profile {
            return Err(SnapshotError::Profile);
        }
        if !remainder.is_empty()
            || !state.control.valid_snapshot(state.profile)
            || state.profile == CpuProfile::I8086 && state.a20
        {
            return Err(SnapshotError::Metadata);
        }
        if state.mapping != self.snapshot_mapping() {
            return Err(SnapshotError::Mapping);
        }
        if ram.len() != self.memory.len() {
            return Err(SnapshotError::MemorySize);
        }

        self.memory.copy_from_slice(ram);
        self.regs = state.registers;
        self.control = state.control;
        self.set_a20(state.a20);
        self.ip_high = state.ip_high;
        self.high = state.high;
        self.fs = state.fs;
        self.gs = state.gs;
        self.interrupt_shadow = state.interrupt_shadow;
        self.halted = state.halted;
        self.page_fault = None;
        self.segment_fault = None;
        Ok(())
    }

    fn snapshot_mapping(&self) -> Option<(u32, u32)> {
        self.mapping
            .as_ref()
            .map(|mapping| (mapping.start as u32, mapping.end as u32))
    }
}
