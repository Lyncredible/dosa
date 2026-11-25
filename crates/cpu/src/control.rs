use super::{Cpu, CpuProfile, Instruction, MEMORY_SIZE, Mnemonic, Register};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Table {
    pub(super) base: u32,
    pub(super) limit: u16,
}

impl Table {
    fn at(self, index: u32, width: u32) -> Option<usize> {
        if index.checked_add(width - 1)? > u32::from(self.limit) {
            return None;
        }
        let address = self.base.checked_add(index)? as usize;
        (address.checked_add(width as usize)? <= MEMORY_SIZE).then_some(address)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Segment {
    base: u32,
    limit: u32,
    readable: bool,
    writable: bool,
    usable: bool,
}

impl Segment {
    const NULL: Self = Self {
        base: 0,
        limit: 0,
        readable: false,
        writable: false,
        usable: false,
    };

    pub(super) fn address(
        self,
        fetch: bool,
        offset: u32,
        width: usize,
        write: bool,
    ) -> Option<usize> {
        let end = offset.checked_add(u32::try_from(width.checked_sub(1)?).ok()?)?;
        if !self.usable
            || end > self.limit
            || write && !self.writable
            || !write && !fetch && !self.readable
        {
            return None;
        }
        let address = self.base.checked_add(offset)? as usize;
        (address.checked_add(width)? <= MEMORY_SIZE).then_some(address)
    }
}

pub(super) struct Control {
    pub(super) cr0: u32,
    gdtr: Table,
    idtr: Table,
    pub(super) segments: [Option<Segment>; 6],
}

impl Control {
    pub(super) fn new(profile: CpuProfile) -> Self {
        Self {
            cr0: if profile == CpuProfile::I386 { 0x10 } else { 0 },
            gdtr: Table { base: 0, limit: 0 },
            idtr: Table {
                base: 0,
                limit: 0x3ff,
            },
            segments: [None; 6],
        }
    }
}

impl Cpu {
    pub fn protected_mode(&self) -> bool {
        self.control.cr0 & 1 != 0
    }

    pub(super) fn descriptor(&self, register: Register, selector: u16) -> Option<Option<Segment>> {
        if !self.protected_mode() {
            return Some(None);
        }
        if selector < 4 && !matches!(register, Register::CS | Register::SS) {
            return Some(Some(Segment::NULL));
        }
        if selector < 8 || selector & 7 != 0 {
            return None; // This interpreter currently runs ring 0 with the GDT.
        }
        let address = self.control.gdtr.at(u32::from(selector), 8)?;
        let low = self.read_mem(address, 4);
        let high = self.read_mem(address + 4, 4);
        let access = (high >> 8) as u8;
        let code = access & 0x08 != 0;
        let readable_code_or_writable_data = access & 0x02 != 0;
        if access & 0xf0 != 0x90 // Present, ring 0, ordinary code or data.
            || high & 0x0040_0000 != 0 // 32-bit defaults require full EIP/ESP support.
            || access & 0x04 != 0 // Conforming code and expand-down data need their own rules.
            || register == Register::CS && !code
            || register == Register::SS && (code || !readable_code_or_writable_data)
            || register != Register::CS && code && !readable_code_or_writable_data
        {
            return None;
        }
        let is_386 = self.profile == CpuProfile::I386;
        let mut base = (low >> 16) | ((high & 0xff) << 16);
        let mut limit = low & 0xffff;
        if is_386 {
            base |= high & 0xff00_0000;
            limit |= high & 0x000f_0000;
            if high & 0x0080_0000 != 0 {
                limit = (limit << 12) | 0x0fff;
            }
        }
        Some(Some(Segment {
            base,
            limit,
            readable: !code || readable_code_or_writable_data,
            writable: !code && readable_code_or_writable_data,
            usable: true,
        }))
    }

    pub(super) fn interrupt_gate(&self, vector: u8) -> Option<(u16, u16, Segment, bool)> {
        let address = self.control.idtr.at(u32::from(vector) * 8, 8)?;
        let low = self.read_mem(address, 4);
        let high = self.read_mem(address + 4, 4);
        let gate = (high >> 8) as u8;
        if !matches!(gate & !0x60, 0x86 | 0x87) {
            return None;
        }
        let selector = (low >> 16) as u16;
        let offset = low as u16;
        let code = self.descriptor(Register::CS, selector)??;
        code.address(true, offset.into(), 1, false)?;
        Some((offset, selector, code, gate & 1 == 0))
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
                let mut base = self.read_mem(address + 2, 4);
                if !wide {
                    base &= 0x00ff_ffff;
                }
                let table = Table {
                    base,
                    limit: self.read_mem(address, 2) as u16,
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
                self.write_mem(address, 2, u32::from(table.limit));
                self.write_mem(address + 2, 4, base);
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
        if value & 0x8000_0000 != 0 {
            return None; // Paging is a separate MMU increment.
        }
        self.control.cr0 = (value & 0x0f) | 0x10;
        Some(())
    }
}
