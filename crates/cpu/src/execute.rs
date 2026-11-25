use super::*;
use iced_x86::{Code, ConditionCode, FlowControl};

impl Cpu {
    pub(super) fn execute(&mut self, ins: &Instruction, start_ip: u16) -> Option<Step> {
        use Mnemonic::*;
        let op = ins.mnemonic();
        match op {
            Mov | Movzx | Movsx => {
                let mut value = self.read_op(ins, 1)?;
                if op == Movsx {
                    let shift = 32 - Self::width(ins, 1)? * 8;
                    value = (((value << shift) as i32) >> shift) as u32;
                }
                self.write_op(ins, 0, value)?;
            }
            Sgdt | Sidt | Lgdt | Lidt => self.table_instruction(ins)?,
            Smsw | Lmsw => self.machine_status_instruction(ins)?,
            Lea => {
                let offset = self.offset(ins)?;
                self.write_op(ins, 0, offset)?;
            }
            Les | Lds | Lss | Lfs | Lgs => {
                let segment = match op {
                    Les => Register::ES,
                    Lds => Register::DS,
                    Lss => Register::SS,
                    Lfs => Register::FS,
                    Lgs => Register::GS,
                    _ => unreachable!(),
                };
                let width = Self::width(ins, 0)?;
                let address = self.memory_address(ins, false)?;
                let offset = self.read_mem(address, width);
                let selector = self.read_mem(address + usize::from(width), 2);
                self.set_reg(segment, selector)?;
                self.write_op(ins, 0, offset)?;
            }
            Xchg => {
                let left = self.read_op(ins, 0)?;
                let right = self.read_op(ins, 1)?;
                if ins.op_kind(1) == OpKind::Memory {
                    self.write_op(ins, 1, left)?;
                    self.write_op(ins, 0, right)?;
                } else {
                    self.write_op(ins, 0, right)?;
                    self.write_op(ins, 1, left)?;
                }
            }
            Xlatb => {
                let value = self.read_mem(self.memory_address(ins, false)?, 1);
                self.set_reg(Register::AL, value)?;
            }
            Add | Adc | Sub | Sbb | And | Or | Xor | Cmp | Test => {
                let left = self.read_op(ins, 0)?;
                let right = self.read_op(ins, 1)?;
                let result = self.binary(op, left, right, Self::width(ins, 0)?);
                if !matches!(op, Cmp | Test) {
                    self.write_op(ins, 0, result)?;
                }
            }
            Inc | Dec | Neg | Not => {
                let operand = self.read_op(ins, 0)?;
                let width = Self::width(ins, 0)?;
                let result = match op {
                    Neg => self.binary(op, 0, operand, width),
                    Not => !operand,
                    _ => self.binary(op, operand, 1, width),
                };
                self.write_op(ins, 0, result)?;
            }
            Shl | Sal | Shr | Sar | Rol | Ror | Rcl | Rcr => {
                let value = self.read_op(ins, 0)?;
                let count = self.read_op(ins, 1)?;
                let result = self.shift(op, value, count, Self::width(ins, 0)?);
                self.write_op(ins, 0, result)?;
            }
            Mul | Imul => self.product(ins)?,
            Div | Idiv => return self.divide(ins, start_ip),
            Cbw => self.regs.ax = i16::from(self.regs.ax as i8) as u16,
            Cwde => self.set_reg(Register::EAX, i32::from(self.regs.ax as i16) as u32)?,
            Cwd => self.regs.dx = ((self.regs.ax as i16) >> 15) as u16,
            Cdq => {
                let signed = self.get_reg(Register::EAX)? as i32;
                self.set_reg(Register::EDX, (signed >> 31) as u32)?;
            }
            Cmc => self.regs.flags ^= CF,
            Clc | Stc => self.set_flag(CF, op == Stc),
            Cli | Sti => self.set_flag(IF, op == Sti),
            Cld | Std => self.set_flag(DF, op == Std),
            Lahf => {
                let status = self.regs.flags & (SF | ZF | AF | PF | CF);
                self.set_reg(Register::AH, u32::from(status | 2))?;
            }
            Sahf => {
                let mask = SF | ZF | AF | PF | CF;
                self.regs.flags = (self.regs.flags & !mask) | ((self.regs.ax >> 8) & mask) | 2;
            }
            Push => {
                let width = push_pop_width(ins)?;
                let value = if self.profile == CpuProfile::I8086
                    && ins.op0_kind() == OpKind::Register
                    && ins.op0_register() == Register::SP
                {
                    // The 8086 pushes the decremented value of SP.
                    self.regs.sp.wrapping_sub(2).into()
                } else {
                    self.read_op(ins, 0)?
                };
                self.push(width, value)?;
            }
            Pop => {
                let value = self.pop(push_pop_width(ins)?)?;
                self.write_op(ins, 0, value)?;
            }
            Pushf | Pushfd => {
                self.push(
                    if op == Pushf { 2 } else { 4 },
                    u32::from(self.profile.normalize_flags(self.regs.flags)),
                )?;
            }
            Popf | Popfd => {
                let flags = self.pop(if op == Popf { 2 } else { 4 })? as u16;
                self.regs.flags = self.profile.normalize_flags(flags);
            }
            Pusha | Pushad => self.push_all(if op == Pusha { 2 } else { 4 })?,
            Popa | Popad => self.pop_all(if op == Popa { 2 } else { 4 })?,
            Enter => self.enter(ins)?,
            Leave => {
                let width = if ins.code() == Code::Leavew { 2 } else { 4 };
                self.regs.sp = self.regs.bp;
                let value = self.pop(width)?;
                self.set_reg(
                    if width == 2 {
                        Register::BP
                    } else {
                        Register::EBP
                    },
                    value,
                )?;
            }
            Jmp | Call => self.jump(ins)?,
            Ret | Retf | Iret | Iretd => self.return_from(ins)?,
            Loop | Loope | Loopne | Jcxz | Jecxz => {
                self.loop_branch(ins)?;
            }
            Int => return Some(Step::Interrupt(ins.immediate8())),
            Int3 => return Some(Step::Interrupt(3)),
            Into if self.flag(OF) => return Some(Step::Interrupt(4)),
            Hlt => return Some(Step::Halt),
            In => {
                return Some(Step::PortRead {
                    port: self.read_op(ins, 1)? as u16,
                    width: Self::width(ins, 0)?,
                });
            }
            Out => {
                return Some(Step::PortWrite {
                    port: self.read_op(ins, 0)? as u16,
                    width: Self::width(ins, 1)?,
                    value: self.read_op(ins, 1)?,
                });
            }
            Movsb | Movsw | Movsd | Cmpsb | Cmpsw | Cmpsd | Lodsb | Lodsw | Lodsd | Stosb
            | Stosw | Stosd | Scasb | Scasw | Scasd => {
                self.string(ins, start_ip)?;
            }
            Nop | Pause | Into | Wait => {}
            _ if ins.flow_control() == FlowControl::ConditionalBranch => {
                if self.condition(ins.condition_code())? {
                    self.regs.ip = ins.near_branch_target() as u16;
                }
            }
            _ => return None,
        }
        Some(Step::Continue)
    }

    fn condition(&self, code: ConditionCode) -> Option<bool> {
        use ConditionCode::*;
        Some(match code {
            o => self.flag(OF),
            no => !self.flag(OF),
            b => self.flag(CF),
            ae => !self.flag(CF),
            e => self.flag(ZF),
            ne => !self.flag(ZF),
            be => self.flag(CF) || self.flag(ZF),
            a => !self.flag(CF) && !self.flag(ZF),
            s => self.flag(SF),
            ns => !self.flag(SF),
            p => self.flag(PF),
            np => !self.flag(PF),
            l => self.flag(SF) != self.flag(OF),
            ge => self.flag(SF) == self.flag(OF),
            le => self.flag(ZF) || self.flag(SF) != self.flag(OF),
            g => !self.flag(ZF) && self.flag(SF) == self.flag(OF),
            _ => return Option::None,
        })
    }

    fn jump(&mut self, ins: &Instruction) -> Option<()> {
        let (target, segment, width) = match ins.op0_kind() {
            OpKind::NearBranch16 => (ins.near_branch16().into(), None, 2),
            OpKind::NearBranch32 => (ins.near_branch32(), None, 4),
            OpKind::FarBranch16 => (
                ins.far_branch16().into(),
                Some(ins.far_branch_selector()),
                2,
            ),
            OpKind::FarBranch32 => (ins.far_branch32(), Some(ins.far_branch_selector()), 4),
            OpKind::Memory
                if matches!(
                    ins.code(),
                    Code::Call_m1616 | Code::Call_m1632 | Code::Jmp_m1616 | Code::Jmp_m1632
                ) =>
            {
                let width = if matches!(ins.code(), Code::Call_m1616 | Code::Jmp_m1616) {
                    2
                } else {
                    4
                };
                let address = self.memory_address(ins, false)?;
                let segment = self.read_mem(address + usize::from(width), 2) as u16;
                (self.read_mem(address, width), Some(segment), width)
            }
            OpKind::Register | OpKind::Memory => {
                (self.read_op(ins, 0)?, None, Self::width(ins, 0)?)
            }
            _ => return None,
        };
        let target = u16::try_from(target).ok()?;
        let descriptor = if let Some(segment) = segment {
            let descriptor = self.descriptor(Register::CS, segment)?;
            if let Some(descriptor) = descriptor {
                descriptor.address(true, target.into(), 1, false)?;
            }
            descriptor
        } else {
            None
        };
        if ins.mnemonic() == Mnemonic::Call {
            self.check_stack_push(width, 1 + u16::from(segment.is_some()))?;
            if segment.is_some() {
                self.push(width, self.regs.cs.into())?;
            }
            self.push(width, self.regs.ip.into())?;
        }
        if let Some(segment) = segment {
            self.control.segments[Register::CS.number()] = descriptor;
            self.regs.cs = segment;
        }
        self.regs.ip = target;
        Some(())
    }

    fn return_from(&mut self, ins: &Instruction) -> Option<()> {
        let width = match ins.code() {
            Code::Retnw | Code::Retnw_imm16 | Code::Retfw | Code::Retfw_imm16 | Code::Iretw => 2,
            Code::Retnd | Code::Retnd_imm16 | Code::Retfd | Code::Retfd_imm16 | Code::Iretd => 4,
            _ => return None,
        };
        let target = u16::try_from(self.peek_stack(width, 0)?).ok()?;
        let far = matches!(
            ins.mnemonic(),
            Mnemonic::Retf | Mnemonic::Iret | Mnemonic::Iretd
        );
        let selector = if far {
            Some(self.peek_stack(width, width.into())? as u16)
        } else {
            None
        };
        let interrupt = matches!(ins.mnemonic(), Mnemonic::Iret | Mnemonic::Iretd);
        let flags = if interrupt {
            Some(self.peek_stack(width, u16::from(width) * 2)? as u16)
        } else {
            None
        };
        if let Some(selector) = selector {
            let descriptor = self.descriptor(Register::CS, selector)?;
            if let Some(descriptor) = descriptor {
                descriptor.address(true, target.into(), 1, false)?;
            }
            self.control.segments[Register::CS.number()] = descriptor;
            self.regs.cs = selector;
        }
        self.regs.ip = target;
        let words = 1 + u16::from(far) + u16::from(interrupt);
        self.regs.sp = self.regs.sp.wrapping_add(words * u16::from(width));
        if let Some(flags) = flags {
            self.regs.flags = self.profile.normalize_flags(flags);
        }
        if ins.op_count() == 1 {
            self.regs.sp = self.regs.sp.wrapping_add(ins.immediate16());
        }
        Some(())
    }

    fn loop_branch(&mut self, ins: &Instruction) -> Option<()> {
        use Mnemonic::*;
        let counter = match ins.code() {
            Code::Loop_rel8_16_ECX
            | Code::Loope_rel8_16_ECX
            | Code::Loopne_rel8_16_ECX
            | Code::Loop_rel8_32_ECX
            | Code::Loope_rel8_32_ECX
            | Code::Loopne_rel8_32_ECX => Register::ECX,
            _ if ins.mnemonic() == Jecxz => Register::ECX,
            _ => Register::CX,
        };
        let mut count = self.get_reg(counter)?;
        let jump = match ins.mnemonic() {
            Jcxz | Jecxz => count == 0,
            op => {
                count = count.wrapping_sub(1) & super::arithmetic::mask(counter.size() as u8);
                self.set_reg(counter, count)?;
                count != 0 && (op == Loop || self.flag(ZF) == (op == Loope))
            }
        };
        if jump {
            self.regs.ip = u16::try_from(ins.near_branch_target()).ok()?;
        }
        Some(())
    }

    fn push_all(&mut self, width: u8) -> Option<()> {
        self.check_stack_push(width, 8)?;
        let registers = general_registers(width);
        let sp = self.get_reg(registers[4])?;
        for (index, reg) in registers.into_iter().enumerate() {
            let value = if index == 4 { sp } else { self.get_reg(reg)? };
            self.push(width, value)?;
        }
        Some(())
    }

    fn pop_all(&mut self, width: u8) -> Option<()> {
        for (index, reg) in general_registers(width).into_iter().enumerate().rev() {
            let value = self.pop(width)?;
            if index != 4 {
                self.set_reg(reg, value)?;
            }
        }
        Some(())
    }

    fn enter(&mut self, ins: &Instruction) -> Option<()> {
        let width = if ins.code() == Code::Enterw_imm16_imm8 {
            2
        } else {
            4
        };
        let nesting = ins.immediate8_2nd() & 31;
        let register = if width == 2 {
            Register::BP
        } else {
            Register::EBP
        };
        self.push(width, self.get_reg(register)?)?;
        let frame = self.regs.sp;
        if nesting > 0 {
            let mut parent = self.regs.bp;
            for _ in 1..nesting {
                parent = parent.wrapping_sub(width.into());
                let address =
                    self.segment_address(Register::SS, parent.into(), usize::from(width), false)?;
                let saved = self.read_mem(address, width);
                self.push(width, saved)?;
            }
            self.push(width, frame.into())?;
        }
        self.set_reg(register, frame.into())?;
        self.regs.sp = self.regs.sp.wrapping_sub(ins.immediate16());
        Some(())
    }

    fn string(&mut self, ins: &Instruction, start_ip: u16) -> Option<()> {
        use Mnemonic::*;
        let width = match ins.code() {
            Code::Movsb_m8_m8
            | Code::Cmpsb_m8_m8
            | Code::Lodsb_AL_m8
            | Code::Stosb_m8_AL
            | Code::Scasb_AL_m8 => 1,
            Code::Movsw_m16_m16
            | Code::Cmpsw_m16_m16
            | Code::Lodsw_AX_m16
            | Code::Stosw_m16_AX
            | Code::Scasw_AX_m16 => 2,
            Code::Movsd_m32_m32
            | Code::Cmpsd_m32_m32
            | Code::Lodsd_EAX_m32
            | Code::Stosd_m32_EAX
            | Code::Scasd_EAX_m32 => 4,
            _ => return None,
        };
        let mut source = None;
        let mut dest = None;
        for kind in ins.op_kinds() {
            match kind {
                OpKind::MemorySegSI => source = Some(Register::SI),
                OpKind::MemorySegESI => source = Some(Register::ESI),
                OpKind::MemoryESDI => dest = Some(Register::DI),
                OpKind::MemoryESEDI => dest = Some(Register::EDI),
                _ => {}
            }
        }
        let counter = if source == Some(Register::ESI) || dest == Some(Register::EDI) {
            Register::ECX
        } else {
            Register::CX
        };
        let repeat = ins.has_rep_prefix() || ins.has_repne_prefix();
        let count = self.get_reg(counter)?;
        if repeat && count == 0 {
            return Some(());
        }
        let bytes = usize::from(width);
        let src_addr = if let Some(register) = source {
            self.segment_address(ins.memory_segment(), self.get_reg(register)?, bytes, false)?
        } else {
            0
        };
        let dst_addr = if let Some(register) = dest {
            let write = matches!(
                ins.mnemonic(),
                Movsb | Movsw | Movsd | Stosb | Stosw | Stosd
            );
            self.segment_address(Register::ES, self.get_reg(register)?, bytes, write)?
        } else {
            0
        };
        let accumulator = Self::accumulator(width)?;
        let is_comparison = matches!(
            ins.mnemonic(),
            Cmpsb | Cmpsw | Cmpsd | Scasb | Scasw | Scasd
        );
        match ins.mnemonic() {
            Movsb | Movsw | Movsd => {
                self.write_mem(dst_addr, width, self.read_mem(src_addr, width))
            }
            Cmpsb | Cmpsw | Cmpsd => {
                self.binary(
                    Cmp,
                    self.read_mem(src_addr, width),
                    self.read_mem(dst_addr, width),
                    width,
                );
            }
            Lodsb | Lodsw | Lodsd => self.set_reg(accumulator, self.read_mem(src_addr, width))?,
            Stosb | Stosw | Stosd => self.write_mem(dst_addr, width, self.get_reg(accumulator)?),
            Scasb | Scasw | Scasd => {
                self.binary(
                    Cmp,
                    self.get_reg(accumulator)?,
                    self.read_mem(dst_addr, width),
                    width,
                );
            }
            _ => return None,
        }
        let step = if self.flag(DF) {
            0u32.wrapping_sub(width.into())
        } else {
            width.into()
        };
        for reg in [source, dest].into_iter().flatten() {
            self.set_reg(reg, self.get_reg(reg)?.wrapping_add(step))?;
        }
        if repeat {
            self.set_reg(counter, count - 1)?;
            if count > 1 && (!is_comparison || self.flag(ZF) != ins.has_repne_prefix()) {
                self.regs.ip = start_ip;
            }
        }
        Some(())
    }
}

fn push_pop_width(ins: &Instruction) -> Option<u8> {
    match ins.stack_pointer_increment().unsigned_abs() {
        width @ (2 | 4) => Some(width as u8),
        _ => None,
    }
}

fn general_registers(width: u8) -> [Register; 8] {
    use Register::*;
    if width == 2 {
        [AX, CX, DX, BX, SP, BP, SI, DI]
    } else {
        [EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI]
    }
}
