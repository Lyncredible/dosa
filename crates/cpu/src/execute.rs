use super::control::{IOPL, VM};
use super::*;
use iced_x86::{Code, ConditionCode, FlowControl};

impl Cpu {
    pub(super) fn execute(&mut self, ins: &Instruction, start_ip: u32) -> Option<Step> {
        use Mnemonic::*;
        let op = ins.mnemonic();
        if self.control.virtual_8086 {
            match op {
                // Virtual port I/O needs the task's I/O permission bitmap.
                Hlt | Lgdt | Lidt | Lmsw | Clts | Lldt | Ltr | In | Out => {
                    return self.segment_fault(13, 0);
                }
                Lsl | Lar | Sldt | Str | Verr | Verw => return self.segment_fault(6, 0),
                Cli | Sti | Int | Iret | Iretd | Pushf | Pushfd | Popf | Popfd
                    if self.regs.flags & IOPL != IOPL =>
                {
                    return self.segment_fault(13, 0);
                }
                Mov if ins.op0_register().is_cr() || ins.op1_register().is_cr() => {
                    return self.segment_fault(13, 0);
                }
                _ => {}
            }
        }
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
            Lldt | Sldt => self.ldt_instruction(ins)?,
            Ltr | Str => self.task_register_instruction(ins)?,
            Clts => {
                if self.protected_mode() && self.regs.cs & 3 != 0 {
                    return self.segment_fault(13, 0);
                }
                self.control.cr0 &= !(1 << 3);
            }
            Lar | Lsl | Verr | Verw => self.verify_selector(ins)?,
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
                let offset = self.read_linear(address, width)?;
                let selector = self.read_linear(address + usize::from(width), 2)?;
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
                let address = self.memory_address(ins, false)?;
                let value = self.read_linear(address, 1)?;
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
            Shld | Shrd => self.double_shift(ins)?,
            Bt | Bts | Btr | Btc => self.bit_test(ins)?,
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
                self.restore_interrupt_flags(flags);
            }
            Pusha | Pushad => self.push_all(if op == Pusha { 2 } else { 4 })?,
            Popa | Popad => self.pop_all(if op == Popa { 2 } else { 4 })?,
            Enter => self.enter(ins)?,
            Leave => {
                let width = if ins.code() == Code::Leavew { 2 } else { 4 };
                let frame_pointer = if self.wide_stack() {
                    Register::EBP
                } else {
                    Register::BP
                };
                self.set_stack_pointer(self.get_reg(frame_pointer)?);
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
            Seto | Setno | Setb | Setae | Sete | Setne | Setbe | Seta | Sets | Setns | Setp
            | Setnp | Setl | Setge | Setle | Setg => {
                let value = self.condition(ins.condition_code())?;
                self.write_op(ins, 0, u32::from(value))?;
            }
            _ if ins.flow_control() == FlowControl::ConditionalBranch => {
                if self.condition(ins.condition_code())? {
                    self.near_transfer(ins.near_branch_target() as u32)?;
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

    fn near_transfer(&mut self, target: u32) -> Option<()> {
        self.check_near_target(target)?;
        self.set_ip(target);
        Some(())
    }

    fn check_near_target(&mut self, target: u32) -> Option<()> {
        if !self.control.virtual_8086
            && self.control.segments[Register::CS.number()].is_none()
            && target > 0xffff
        {
            return None;
        }
        if self.code_address(target, 1).is_none() {
            return self.segment_fault(13, 0);
        }
        Some(())
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
                let segment = self.read_linear(address + usize::from(width), 2)? as u16;
                (self.read_linear(address, width)?, Some(segment), width)
            }
            OpKind::Register | OpKind::Memory => {
                (self.read_op(ins, 0)?, None, Self::width(ins, 0)?)
            }
            _ => return None,
        };

        let descriptor = if let Some(segment) = segment {
            self.check_far_target(segment, target)?
        } else {
            self.check_near_target(target)?;
            None
        };
        if ins.mnemonic() == Mnemonic::Call {
            self.check_stack_push(width, 1 + u8::from(segment.is_some()))?;
            if segment.is_some() {
                self.push(width, self.regs.cs.into())?;
            }
            self.push(width, self.instruction_pointer())?;
        }
        if let Some(segment) = segment {
            self.control.segments[Register::CS.number()] = descriptor;
            self.regs.cs = segment;
        }
        self.set_ip(target);
        Some(())
    }

    fn check_far_target(&mut self, selector: u16, target: u32) -> Option<Option<Segment>> {
        let descriptor = self.descriptor(Register::CS, selector)?;
        if let Some(code) = descriptor {
            if code.address(true, target, 1, false).is_none() {
                return self.segment_fault(13, 0);
            }
        } else if target > 0xffff {
            return self.segment_fault(13, 0);
        }
        Some(descriptor)
    }

    fn return_from(&mut self, ins: &Instruction) -> Option<()> {
        let width = match ins.code() {
            Code::Retnw | Code::Retnw_imm16 | Code::Retfw | Code::Retfw_imm16 | Code::Iretw => 2,
            Code::Retnd | Code::Retnd_imm16 | Code::Retfd | Code::Retfd_imm16 | Code::Iretd => 4,
            _ => return None,
        };
        let target = self.peek_stack(width, 0)?;
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
            Some(self.peek_stack(width, u32::from(width) * 2)?)
        } else {
            None
        };
        if let Some(selector) = selector {
            if let Some(flags) = flags
                && self.protected_mode()
                && !self.control.virtual_8086
                && self.regs.cs & 3 == 0
                && flags & VM != 0
            {
                return self.return_to_virtual_8086(target, selector, flags);
            }
            let descriptor = self.check_far_target(selector, target)?;
            self.control.segments[Register::CS.number()] = descriptor;
            self.regs.cs = selector;
        } else {
            self.check_near_target(target)?;
        }
        self.set_ip(target);
        let words = 1 + u32::from(far) + u32::from(interrupt);
        self.adjust_stack(words * u32::from(width));
        if let Some(flags) = flags {
            self.restore_interrupt_flags(flags as u16);
        }
        if ins.op_count() == 1 {
            self.adjust_stack(ins.immediate16().into());
        }
        Some(())
    }

    fn restore_interrupt_flags(&mut self, flags: u16) {
        let flags = if self.control.virtual_8086 {
            (flags & !IOPL) | (self.regs.flags & IOPL)
        } else {
            flags
        };
        self.regs.flags = self.profile.normalize_flags(flags);
    }

    fn return_to_virtual_8086(&mut self, target: u32, cs: u16, flags: u32) -> Option<()> {
        if self.regs.flags & 0x4000 != 0 {
            // A nested-task return requires a hardware task switch.
            return None;
        }
        if target > u32::from(u16::MAX) {
            return self.segment_fault(13, 0);
        }
        let esp = self.peek_stack(4, 12)?;
        let ss = self.peek_stack(4, 16)? as u16;
        let es = self.peek_stack(4, 20)? as u16;
        let ds = self.peek_stack(4, 24)? as u16;
        let fs = self.peek_stack(4, 28)? as u16;
        let gs = self.peek_stack(4, 32)? as u16;

        self.set_reg(Register::ESP, esp)?;
        self.set_ip(target);
        self.regs.cs = cs;
        self.regs.ss = ss;
        self.regs.es = es;
        self.regs.ds = ds;
        self.fs = fs;
        self.gs = gs;
        self.regs.flags = self.profile.normalize_flags(flags as u16);
        self.control.segments = [None; 6];
        self.control.virtual_8086 = true;
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
            self.near_transfer(ins.near_branch_target() as u32)?;
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
        let frame = if width == 2 {
            self.get_reg(Register::SP)?
        } else {
            self.get_reg(Register::ESP)?
        };
        if nesting > 0 {
            let frame_pointer = if self.wide_stack() {
                Register::EBP
            } else {
                Register::BP
            };
            let mask = if self.wide_stack() { u32::MAX } else { 0xffff };
            let mut parent = self.get_reg(frame_pointer)?;
            for _ in 1..nesting {
                parent = parent.wrapping_sub(u32::from(width)) & mask;
                let address =
                    self.segment_access(Register::SS, parent, usize::from(width), false)?;
                let saved = self.read_linear(address, width)?;
                self.push(width, saved)?;
            }
            self.push(width, frame)?;
        }
        self.set_reg(register, frame)?;
        self.adjust_stack(0u32.wrapping_sub(ins.immediate16().into()));
        Some(())
    }

    fn string(&mut self, ins: &Instruction, start_ip: u32) -> Option<()> {
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
            self.segment_access(ins.memory_segment(), self.get_reg(register)?, bytes, false)?
        } else {
            0
        };
        let dst_addr = if let Some(register) = dest {
            let write = matches!(
                ins.mnemonic(),
                Movsb | Movsw | Movsd | Stosb | Stosw | Stosd
            );
            self.segment_access(Register::ES, self.get_reg(register)?, bytes, write)?
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
                let value = self.read_linear(src_addr, width)?;
                self.write_linear(dst_addr, width, value)?;
            }
            Cmpsb | Cmpsw | Cmpsd => {
                let left = self.read_linear(src_addr, width)?;
                let right = self.read_linear(dst_addr, width)?;
                self.binary(Cmp, left, right, width);
            }
            Lodsb | Lodsw | Lodsd => {
                let value = self.read_linear(src_addr, width)?;
                self.set_reg(accumulator, value)?;
            }
            Stosb | Stosw | Stosd => {
                self.write_linear(dst_addr, width, self.get_reg(accumulator)?)?;
            }
            Scasb | Scasw | Scasd => {
                let value = self.read_linear(dst_addr, width)?;
                self.binary(Cmp, self.get_reg(accumulator)?, value, width);
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
                self.set_ip(start_ip);
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
