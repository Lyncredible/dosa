use super::{Cpu, CpuProfile, Instruction, Mnemonic, Register};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Table {
    pub(super) base: u32,
    pub(super) limit: u32,
}

impl Table {
    fn at(self, index: u32, width: u32) -> Option<usize> {
        if index.checked_add(width - 1)? > self.limit {
            return None;
        }
        Some(self.base.wrapping_add(index) as usize)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Segment {
    base: u32,
    limit: u32,
    readable: bool,
    writable: bool,
    usable: bool,
    expand_down: bool,
    pub(super) wide: bool,
}

impl Segment {
    const NULL: Self = Self {
        base: 0,
        limit: 0,
        readable: false,
        writable: false,
        usable: false,
        expand_down: false,
        wide: false,
    };

    pub(super) fn address(
        self,
        fetch: bool,
        offset: u32,
        width: usize,
        write: bool,
    ) -> Option<usize> {
        let end = offset.checked_add(u32::try_from(width.checked_sub(1)?).ok()?)?;
        let outside = if self.expand_down {
            offset <= self.limit || (!self.wide && end > 0xffff)
        } else {
            end > self.limit
        };
        if !self.usable || outside || write && !self.writable || !write && !fetch && !self.readable
        {
            return None;
        }
        Some(self.base.wrapping_add(offset) as usize)
    }
}

pub(super) struct InterruptGate {
    pub(super) offset: u32,
    pub(super) selector: u16,
    pub(super) code: Segment,
    pub(super) width: u8,
    pub(super) disable_irq: bool,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(super) struct Control {
    pub(super) cr0: u32,
    pub(super) cr2: u32,
    pub(super) cr3: u32,
    gdtr: Table,
    idtr: Table,
    ldtr: u16,
    ldt: Option<Table>,
    pub(super) segments: [Option<Segment>; 6],
}

impl Control {
    pub(super) fn valid_snapshot(&self, profile: CpuProfile) -> bool {
        let valid_cr0 = match profile {
            CpuProfile::I8086 => self.cr0 == 0 && self.segments.iter().all(Option::is_none),
            CpuProfile::I286 => self.cr0 & !0xf == 0,
            CpuProfile::I386 => {
                self.cr0 & !(super::paging::PG | 0x1f) == 0
                    && self.cr0 & 0x10 != 0
                    && (self.cr0 & super::paging::PG == 0 || self.cr0 & 1 != 0)
            }
        };
        let valid_ldt = if self.ldt.is_some() {
            self.ldtr & 4 == 0 && self.ldtr & !7 != 0
        } else {
            self.ldtr & !3 == 0
        };
        valid_cr0
            && valid_ldt
            && self.gdtr.limit <= u32::from(u16::MAX)
            && self.idtr.limit <= u32::from(u16::MAX)
            && self.cr3 & 0xfff == 0
            && (profile == CpuProfile::I386 || self.cr2 == 0 && self.cr3 == 0)
    }

    pub(super) fn new(profile: CpuProfile) -> Self {
        Self {
            cr0: if profile == CpuProfile::I386 { 0x10 } else { 0 },
            cr2: 0,
            cr3: 0,
            gdtr: Table { base: 0, limit: 0 },
            idtr: Table {
                base: 0,
                limit: 0x3ff,
            },
            ldtr: 0,
            ldt: None,
            segments: [None; 6],
        }
    }
}

impl Cpu {
    pub fn protected_mode(&self) -> bool {
        self.control.cr0 & 1 != 0
    }

    pub(super) fn descriptor(
        &mut self,
        register: Register,
        selector: u16,
    ) -> Option<Option<Segment>> {
        if !self.protected_mode() {
            return Some(None);
        }
        let table_selector = selector & !3;
        let index = selector & !7;
        let rpl = (selector & 3) as u8;
        let local = selector & 4 != 0;
        if index == 0 && !local {
            return if matches!(register, Register::CS | Register::SS) {
                self.segment_fault(13, 0)
            } else {
                Some(Some(Segment::NULL))
            };
        }
        let Some(address) = self.selector_address(selector) else {
            return self.segment_fault(13, table_selector);
        };
        let low = self.read_linear(address, 4)?;
        let high = self.read_linear(address + 4, 4)?;
        let access = (high >> 8) as u8;
        let is_386 = self.profile == CpuProfile::I386;
        let wide = high & 0x0040_0000 != 0;
        let code = access & 0x08 != 0;
        let conforming = code && access & 0x04 != 0;
        let read_write = access & 0x02 != 0;
        let dpl = (access >> 5) & 3;
        let permitted = match register {
            Register::CS => code && dpl == 0 && rpl == 0,
            Register::SS => !code && read_write && dpl == 0 && rpl == 0,
            _ => (!code || read_write) && (conforming || rpl <= dpl),
        };
        if access & 0x10 == 0 || wide && !is_386 || !permitted {
            return self.segment_fault(13, table_selector);
        }
        if access & 0x80 == 0 {
            let vector = if register == Register::SS { 12 } else { 11 };
            return self.segment_fault(vector, table_selector);
        }
        let Table { base, limit } = self.descriptor_span(low, high);
        Some(Some(Segment {
            base,
            limit,
            readable: !code || read_write,
            writable: !code && read_write,
            usable: true,
            expand_down: !code && access & 0x04 != 0,
            wide,
        }))
    }

    fn selector_address(&self, selector: u16) -> Option<usize> {
        let local = selector & 4 != 0;
        let index = u32::from(selector & !7);
        if local {
            self.control.ldt?.at(index, 8)
        } else if index != 0 {
            self.control.gdtr.at(index, 8)
        } else {
            None
        }
    }

    pub(super) fn verify_selector(&mut self, ins: &Instruction) -> Option<()> {
        if !self.protected_mode() {
            return None;
        }
        use Mnemonic::*;
        let op = ins.mnemonic();
        let loads = matches!(op, Lar | Lsl);
        let selector = self.read_op(ins, u32::from(loads))? as u16;
        let Some(address) = self.selector_address(selector) else {
            self.set_flag(super::ZF, false);
            return Some(());
        };
        let high = self.read_linear(address + 4, 4)?;
        let access = (high >> 8) as u8;
        let ordinary = access & 0x10 != 0;
        let code = access & 8 != 0;
        let conforming = ordinary && code && access & 4 != 0;
        let visible = conforming || (selector as u8 & 3) <= ((access >> 5) & 3);
        let read_write = access & 2 != 0;
        let kind = access & 0x0f;
        let valid = visible
            && match op {
                Verr => ordinary && (!code || read_write),
                Verw => ordinary && !code && read_write,
                Lar => {
                    ordinary
                        || matches!(kind, 1..=5)
                        || self.profile == CpuProfile::I386 && matches!(kind, 9 | 0xb | 0xc)
                }
                Lsl => {
                    ordinary
                        || matches!(kind, 1..=3)
                        || self.profile == CpuProfile::I386 && matches!(kind, 9 | 0xb)
                }
                _ => return None,
            };
        if valid && loads {
            let value = if op == Lar {
                high & 0x00f0_ff00
            } else {
                let low = self.read_linear(address, 4)?;
                self.descriptor_span(low, high).limit
            };
            self.write_op(ins, 0, value)?;
        }
        self.set_flag(super::ZF, valid);
        Some(())
    }

    fn descriptor_span(&self, low: u32, high: u32) -> Table {
        let mut base = (low >> 16) | ((high & 0xff) << 16);
        let mut limit = low & 0xffff;
        if self.profile == CpuProfile::I386 {
            base |= high & 0xff00_0000;
            limit |= high & 0x000f_0000;
            if high & 0x0080_0000 != 0 {
                limit = (limit << 12) | 0x0fff;
            }
        }
        Table { base, limit }
    }

    pub(super) fn ldt_instruction(&mut self, ins: &Instruction) -> Option<()> {
        if !self.protected_mode() {
            return None;
        }
        if ins.mnemonic() == Mnemonic::Sldt {
            return self.write_op(ins, 0, u32::from(self.control.ldtr));
        }

        let selector = self.read_op(ins, 0)? as u16;
        let selector_code = selector & !3;
        let table = if selector_code == 0 {
            None
        } else {
            let address = self.control.gdtr.at(u32::from(selector & !7), 8);
            let Some(address) = address.filter(|_| selector & 4 == 0) else {
                return self.segment_fault(13, selector_code);
            };
            let low = self.read_linear(address, 4)?;
            let high = self.read_linear(address + 4, 4)?;
            let access = (high >> 8) as u8;
            if access & 0x1f != 2 {
                return self.segment_fault(13, selector_code);
            }
            if access & 0x80 == 0 {
                return self.segment_fault(11, selector_code);
            }
            Some(self.descriptor_span(low, high))
        };
        self.control.ldtr = selector;
        self.control.ldt = table;
        Some(())
    }

    pub(super) fn interrupt_gate(&mut self, vector: u8) -> Option<InterruptGate> {
        let address = self.control.idtr.at(u32::from(vector) * 8, 8)?;
        let low = self.read_linear(address, 4)?;
        let high = self.read_linear(address + 4, 4)?;
        let gate = ((high >> 8) as u8) & !0x60;
        let width = match gate {
            0x86 | 0x87 => 2,
            0x8e | 0x8f if self.profile == CpuProfile::I386 => 4,
            _ => return None,
        };
        let selector = (low >> 16) as u16 & !3;
        let offset = (low & 0xffff) | if width == 4 { high & 0xffff_0000 } else { 0 };
        let code = self.descriptor(Register::CS, selector)??;
        code.address(true, offset, 1, false)?;
        Some(InterruptGate {
            offset,
            selector,
            code,
            width,
            disable_irq: gate & 1 == 0,
        })
    }

    pub(super) fn table_instruction(&mut self, ins: &Instruction) -> Option<()> {
        use Mnemonic::*;
        use iced_x86::Code::*;
        let op = ins.mnemonic();
        let address = self.memory_address(ins, matches!(op, Sgdt | Sidt))?;
        let wide = matches!(
            ins.code(),
            Lgdt_m1632 | Lidt_m1632 | Sgdt_m1632 | Sidt_m1632
        );
        match op {
            Lgdt | Lidt => {
                let mut base = self.read_linear(address + 2, 4)?;
                if !wide {
                    base &= 0x00ff_ffff;
                }
                let table = Table {
                    base,
                    limit: self.read_linear(address, 2)?,
                };
                if op == Lgdt {
                    self.control.gdtr = table;
                } else {
                    self.control.idtr = table;
                }
            }
            Sgdt | Sidt => {
                let table = if op == Sgdt {
                    self.control.gdtr
                } else {
                    self.control.idtr
                };
                let base = if self.profile == CpuProfile::I286 {
                    table.base | 0xff00_0000
                } else {
                    table.base
                };
                let mut bytes = [0; 6];
                bytes[..2].copy_from_slice(&(table.limit as u16).to_le_bytes());
                bytes[2..].copy_from_slice(&base.to_le_bytes());
                self.write_linear_bytes(address, &bytes)?;
            }
            _ => return None,
        }
        Some(())
    }

    pub(super) fn machine_status_instruction(&mut self, ins: &Instruction) -> Option<()> {
        if ins.mnemonic() == Mnemonic::Lmsw {
            let input = self.read_op(ins, 0)? & 0xf;
            // LMSW can set PE, but cannot clear it after entering protected mode.
            self.control.cr0 = (self.control.cr0 & !0xe) | input;
            Some(())
        } else {
            self.write_op(ins, 0, self.control.cr0 & 0xffff)
        }
    }

    pub(super) fn write_cr0(&mut self, value: u32) -> Option<()> {
        if value & super::paging::PG != 0 && value & 1 == 0 {
            return None;
        }
        self.control.cr0 = (value & (super::paging::PG | 0x0f)) | 0x10;
        Some(())
    }
}
