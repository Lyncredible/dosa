use crate::{DosError, Environment, UpperProgram, environment, xms};
use dosa_cpu::{Cpu, CpuProfile};

pub(crate) const PSP: u16 = 0x1000;
pub(crate) const CONVENTIONAL_END: u16 = 0xa000;
const ENVIRONMENT: u16 = PSP - (environment::CAPACITY / 16) as u16;

pub(crate) fn load(
    bytes: &[u8],
    name: &str,
    args: &str,
    profile: CpuProfile,
    environment: &Environment,
    upper_program: Option<UpperProgram>,
) -> Result<Cpu, DosError> {
    let mut cpu = Cpu::with_profile(profile);
    let args = if args.is_empty() {
        String::new()
    } else {
        format!(" {args}")
    };
    if args.len() > 126 || !args.is_ascii() || args.contains(['\0', '\r', '\n']) {
        return Err(DosError::InvalidCommandTail);
    }

    let (psp, segment_end) = upper_program
        .map(|upper| (xms::UPPER_START, upper.end))
        .unwrap_or((PSP, CONVENTIONAL_END));
    cpu.load(ENVIRONMENT, 0, &environment.encode(name)?);
    cpu.load(psp, 0, &[0xcd, 0x20]);
    cpu.write_u16(psp, 2, segment_end);
    cpu.write_u16(psp, 0x16, psp);
    cpu.load(psp, 0x18, &[0, 1, 2, 3, 4]);
    cpu.load(psp, 0x1d, &[0xff; 15]);
    cpu.write_u16(psp, 0x2c, ENVIRONMENT);
    cpu.write_u16(psp, 0x32, 20);
    cpu.write_u16(psp, 0x34, 0x18);
    cpu.write_u16(psp, 0x36, psp);
    cpu.load(psp, 0x50, &[0xcd, 0x21, 0xcb]);
    cpu.write_u8(psp, 0x80, args.len() as u8);
    cpu.load(psp, 0x81, args.as_bytes());
    cpu.write_u8(psp, 0x81 + args.len() as u16, b'\r');
    cpu.regs.ds = psp;
    cpu.regs.es = psp;
    cpu.regs.flags = 0x0202;

    if bytes.starts_with(b"MZ") || bytes.starts_with(b"ZM") {
        load_mz(&mut cpu, bytes, psp, segment_end)?;
    } else if name.to_ascii_uppercase().ends_with(".COM") {
        let stack = (usize::from(segment_end - psp) * 16).min(0x10000) - 2;
        if bytes.is_empty() || bytes.len() > stack - 0x100 {
            return Err(DosError::InvalidProgram("invalid COM image size"));
        }
        cpu.load(psp, 0x100, bytes);
        cpu.regs.cs = psp;
        cpu.regs.ip = 0x100;
        cpu.regs.ss = psp;
        cpu.regs.sp = stack as u16;
        cpu.write_u16(psp, stack as u16, 0);
    } else {
        return Err(DosError::InvalidProgram("expected a COM or DOS MZ image"));
    }
    Ok(cpu)
}

fn load_mz(cpu: &mut Cpu, bytes: &[u8], psp: u16, segment_end: u16) -> Result<(), DosError> {
    const HEADER_ERROR: DosError = DosError::InvalidProgram("invalid MZ header");
    if bytes.len() < 28 {
        return Err(HEADER_ERROR);
    }
    let word = |at| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let last_page = usize::from(word(2));
    let pages = usize::from(word(4));
    let relocation_count = usize::from(word(6));
    let header = usize::from(word(8)) * 16;
    let minimum_extra = usize::from(word(0x0a)) * 16;
    let relocations = usize::from(word(0x18));
    let load = psp + 0x10;
    let capacity = usize::from(segment_end - load) * 16;
    if pages == 0
        || last_page >= 512
        || header < 28
        || relocations + relocation_count * 4 > header
        || (relocation_count > 0 && relocations < 28)
        || word(0x1a) != 0
    {
        return Err(HEADER_ERROR);
    }
    let end = if last_page == 0 {
        pages * 512
    } else {
        (pages - 1) * 512 + last_page
    };
    if header >= end || end > bytes.len() || end - header + minimum_extra > capacity {
        return Err(HEADER_ERROR);
    }
    let image = &bytes[header..end];
    let (ss, sp, ip, cs) = (word(0x0e), word(0x10), word(0x14), word(0x16));
    let absolute = |segment: u16, offset: u16| usize::from(segment) * 16 + usize::from(offset);
    let base = absolute(load, 0);
    let entry = absolute(load.wrapping_add(cs), ip);
    let stack = absolute(load.wrapping_add(ss), sp.wrapping_sub(2));
    if !(base..base + image.len()).contains(&entry)
        || !(absolute(psp, 0)..absolute(segment_end, 0) - 1).contains(&stack)
    {
        return Err(DosError::InvalidProgram(
            "MZ entry point or stack is outside load memory",
        ));
    }
    cpu.memory[base..base + image.len()].copy_from_slice(image);
    for relocation in (relocations..relocations + relocation_count * 4).step_by(4) {
        let offset = word(relocation);
        let segment = word(relocation + 2);
        if absolute(segment, offset) + 1 >= image.len() {
            return Err(DosError::InvalidProgram("MZ relocation is outside image"));
        }
        let at = base + absolute(segment, offset);
        let value = u16::from_le_bytes([cpu.memory[at], cpu.memory[at + 1]]).wrapping_add(load);
        cpu.memory[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    cpu.regs.ss = load.wrapping_add(ss);
    cpu.regs.sp = sp;
    cpu.regs.cs = load.wrapping_add(cs);
    cpu.regs.ip = ip;
    Ok(())
}
