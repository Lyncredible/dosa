use super::*;

pub(super) fn mask(width: u8) -> u32 {
    u32::MAX >> (32 - width * 8)
}

fn sign_bit(width: u8) -> u32 {
    1 << (width * 8 - 1)
}

fn signed(value: u32, width: u8) -> i64 {
    let shift = 32 - width * 8;
    i64::from((value.wrapping_shl(shift.into()) as i32) >> shift)
}

impl Cpu {
    fn set_szp(&mut self, value: u32, width: u8) {
        self.set_flag(SF, value & sign_bit(width) != 0);
        self.set_flag(ZF, value & mask(width) == 0);
        self.set_flag(PF, (value as u8).count_ones().is_multiple_of(2));
    }

    pub(super) fn binary(&mut self, op: Mnemonic, a: u32, b: u32, width: u8) -> u32 {
        use Mnemonic::*;
        let a = a & mask(width);
        let b = b & mask(width);
        let carry = u32::from(matches!(op, Adc | Sbb) && self.flag(CF));
        let result = match op {
            Add | Inc | Adc => {
                let wide = u64::from(a) + u64::from(b) + u64::from(carry);
                let result = wide as u32;
                if op != Inc {
                    self.set_flag(CF, wide > u64::from(mask(width)));
                }
                self.set_flag(OF, (!(a ^ b) & (a ^ result) & sign_bit(width)) != 0);
                self.set_flag(AF, (a ^ b ^ result) & 0x10 != 0);
                result
            }
            Sub | Sbb | Cmp | Dec | Neg => {
                let result = a.wrapping_sub(b).wrapping_sub(carry);
                if op != Dec {
                    self.set_flag(CF, u64::from(a) < u64::from(b) + u64::from(carry));
                }
                self.set_flag(OF, ((a ^ b) & (a ^ result) & sign_bit(width)) != 0);
                self.set_flag(AF, (a ^ b ^ result) & 0x10 != 0);
                result
            }
            And | Test | Or | Xor => {
                self.set_flag(CF, false);
                self.set_flag(OF, false);
                match op {
                    And | Test => a & b,
                    Or => a | b,
                    Xor => a ^ b,
                    _ => unreachable!(),
                }
            }
            _ => unreachable!(),
        };
        self.set_szp(result, width);
        result & mask(width)
    }

    pub(super) fn bit_test(&mut self, instruction: &Instruction) -> Option<()> {
        use Mnemonic::*;

        let operation = instruction.mnemonic();
        let width = Self::width(instruction, 0)?;
        let bits = u32::from(width) * 8;
        let index = self.read_op(instruction, 1)?;
        let bit = 1 << (index & (bits - 1));
        let address = if instruction.op0_kind() == OpKind::Memory {
            let adjustment = if instruction.op1_kind() == OpKind::Register {
                (signed(index, width).div_euclid(i64::from(bits)) * i64::from(width)) as i32
            } else {
                0
            };
            let offset = self.adjusted_offset(instruction, adjustment)?;
            Some(self.segment_access(
                instruction.memory_segment(),
                offset,
                usize::from(width),
                operation != Bt,
            )?)
        } else {
            None
        };
        let original = if let Some(address) = address {
            self.read_linear(address, width)?
        } else {
            self.read_op(instruction, 0)?
        };
        let result = match operation {
            Bt => None,
            Bts => Some(original | bit),
            Btr => Some(original & !bit),
            Btc => Some(original ^ bit),
            _ => unreachable!(),
        };
        if let Some(result) = result {
            if let Some(address) = address {
                self.write_linear(address, width, result)?;
            } else {
                self.write_op(instruction, 0, result)?;
            }
        }
        self.set_flag(CF, original & bit != 0);
        Some(())
    }

    pub(super) fn double_shift(&mut self, instruction: &Instruction) -> Option<()> {
        let count = self.read_op(instruction, 2)? & 31;
        if count == 0 {
            return Some(());
        }
        let width = Self::width(instruction, 0)?;
        let bits = u32::from(width) * 8;
        let destination = self.read_op(instruction, 0)?;
        let source = self.read_op(instruction, 1)?;
        let (shifted, carry) = if instruction.mnemonic() == Mnemonic::Shld {
            let combined = (u64::from(destination) << bits) | u64::from(source);
            (combined << count >> bits, combined >> (2 * bits - count))
        } else {
            let combined = (u64::from(source) << bits) | u64::from(destination);
            (combined >> count, combined >> (count - 1))
        };
        let result = shifted as u32 & mask(width);
        self.write_op(instruction, 0, result)?;
        self.set_flag(CF, carry & 1 != 0);
        self.set_szp(result, width);
        if count == 1 {
            self.set_flag(OF, (destination ^ result) & sign_bit(width) != 0);
        }
        Some(())
    }

    pub(super) fn shift(&mut self, op: Mnemonic, mut value: u32, count: u32, width: u8) -> u32 {
        use Mnemonic::*;
        let bits = u32::from(width * 8);
        let sign = sign_bit(width);
        let count = if self.profile == CpuProfile::I8086 {
            count & 0xff
        } else {
            count & 31
        };
        let iterations = match op {
            Rol | Ror => count % bits,
            Rcl | Rcr => count % (bits + 1),
            _ => count,
        };
        if iterations == 0 {
            if count != 0 && matches!(op, Rol | Ror) {
                self.set_flag(CF, value & if op == Rol { 1 } else { sign } != 0);
            }
            return value;
        }
        let original = value;
        let mut carry = self.flag(CF);
        for _ in 0..iterations {
            let high = value & sign != 0;
            let low = value & 1 != 0;
            (value, carry) = match op {
                Shl | Sal => (value << 1, high),
                Shr => (value >> 1, low),
                Sar => ((value >> 1) | (value & sign), low),
                Rol => ((value << 1) | u32::from(high), high),
                Ror => ((value >> 1) | if low { sign } else { 0 }, low),
                Rcl => ((value << 1) | u32::from(carry), high),
                Rcr => ((value >> 1) | if carry { sign } else { 0 }, low),
                _ => unreachable!(),
            };
            value &= mask(width);
        }
        self.set_flag(CF, carry);
        if matches!(op, Shl | Sal | Shr | Sar) {
            self.set_szp(value, width);
        }
        if count == 1 {
            let high = value & sign != 0;
            let next_high = value & (sign >> 1) != 0;
            self.set_flag(
                OF,
                match op {
                    Shl | Sal | Rol | Rcl => high ^ carry,
                    Shr => original & sign != 0,
                    Ror | Rcr => high ^ next_high,
                    Sar => false,
                    _ => unreachable!(),
                },
            );
        }
        value
    }

    pub(super) fn product(&mut self, ins: &Instruction) -> Option<()> {
        use Mnemonic::*;
        let width = Self::width(ins, 0)?;
        if ins.op_count() > 1 {
            let rhs = ins.op_count() - 1;
            let a = self.read_op(ins, rhs - 1)?;
            let b = self.read_op(ins, rhs)?;
            let result = signed(a, width) * signed(b, width);
            self.write_op(ins, 0, result as u32)?;
            let overflow = signed(result as u32, width) != result;
            self.set_flag(CF, overflow);
            self.set_flag(OF, overflow);
            return Some(());
        }

        let accumulator = Self::accumulator(width)?;
        let a = self.get_reg(accumulator)?;
        let b = self.read_op(ins, 0)?;
        let (result, overflow) = if ins.mnemonic() == Imul {
            let result = signed(a, width) * signed(b, width);
            (result as u64, signed(result as u32, width) != result)
        } else {
            let result = u64::from(a) * u64::from(b);
            (result, result > u64::from(mask(width)))
        };
        if width == 1 {
            self.regs.ax = result as u16;
        } else {
            self.set_reg(accumulator, result as u32)?;
            self.set_reg(Self::data_register(width)?, (result >> (width * 8)) as u32)?;
        }
        self.set_flag(CF, overflow);
        self.set_flag(OF, overflow);
        Some(())
    }

    fn data_register(width: u8) -> Option<Register> {
        match width {
            1 => Some(Register::AH),
            2 => Some(Register::DX),
            4 => Some(Register::EDX),
            _ => None,
        }
    }

    pub(super) fn divide(&mut self, ins: &Instruction, ip: u32) -> Option<Step> {
        let width = Self::width(ins, 0)?;
        let accumulator = Self::accumulator(width)?;
        let data = Self::data_register(width)?;
        let bits = width * 8;
        let dividend =
            (u64::from(self.get_reg(data)?) << bits) | u64::from(self.get_reg(accumulator)?);
        let divisor = self.read_op(ins, 0)?;
        let result = if ins.mnemonic() == Mnemonic::Idiv {
            let shift = 64 - 2 * bits;
            let a = ((dividend << shift) as i64) >> shift;
            let b = signed(divisor, width);
            a.checked_div(b)
                .filter(|&q| signed(q as u32, width) == q)
                .map(|q| (q as u32, (a % b) as u32))
        } else {
            let b = u64::from(divisor);
            dividend
                .checked_div(b)
                .filter(|&q| q <= u64::from(mask(width)))
                .map(|q| (q as u32, (dividend % b) as u32))
        };
        let Some((quotient, remainder)) = result else {
            self.set_ip(ip);
            return Some(Step::Interrupt(0));
        };
        self.set_reg(accumulator, quotient)?;
        self.set_reg(data, remainder)?;
        Some(Step::Continue)
    }
}
