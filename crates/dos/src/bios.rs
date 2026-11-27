//! Callable default BIOS interrupt handlers and the BIOS timer data area.

use dosa_cpu::{Cpu, CpuProfile, Register};

use crate::CARRY;
use crate::keyboard::KEYBOARD_VECTOR;
use crate::pit::BIOS_DIVISOR;

pub(super) const TIMER_VECTOR: u8 = 0x08;
const USER_TIMER_VECTOR: u8 = 0x1c;
const ROM: u16 = 0xf000;
// This firmware byte travels with the BIOS tick data in the guest's RAM image.
const RTC_DST_OPTION: u16 = 0x07f0;
const FLOPPY_CHANGE_LINE: u16 = 0x07f1;
const TIMER_ENTRY: u16 = 0x0800;
const TIMER_RETURN: u16 = TIMER_ENTRY + 3;
const KEYBOARD_ENTRY: u16 = 0x0900;
const USER_TIMER_ENTRY: u16 = 0x1c00;
const BDA: u16 = 0x0040;
const EQUIPMENT: u16 = 0x0010;
const COLOR_80_COLUMN_DISPLAY: u16 = 0x0020;
const TICKS: u16 = 0x006c;
const MIDNIGHT: u16 = 0x0070;
const TICKS_PER_DAY: u32 = 0x1800b0;

#[derive(Clone, Copy)]
pub(super) enum Hook {
    Tick,
    TimerEoi,
    Keyboard,
}

pub(super) fn install(cpu: &mut Cpu, now: u64) {
    cpu.write_u16(BDA, EQUIPMENT, COLOR_80_COLUMN_DISPLAY);
    for (vector, entry) in [
        (TIMER_VECTOR, TIMER_ENTRY),
        (KEYBOARD_VECTOR, KEYBOARD_ENTRY),
        (USER_TIMER_VECTOR, USER_TIMER_ENTRY),
    ] {
        let index = u16::from(vector) * 4;
        cpu.write_u16(0, index, entry);
        cpu.write_u16(0, index + 2, ROM);
    }
    // The synthetic firmware keeps a real interrupt frame for chained calls.
    // CLI / INT 1Ch / IRET. The host hooks update ticks and issue EOI.
    cpu.load(ROM, TIMER_ENTRY, &[0xfa, 0xcd, USER_TIMER_VECTOR, 0xcf]);
    cpu.write_u8(ROM, KEYBOARD_ENTRY, 0xcf);
    cpu.write_u8(ROM, USER_TIMER_ENTRY, 0xcf);
    write_ticks(cpu, (now / BIOS_DIVISOR % u64::from(TICKS_PER_DAY)) as u32);
}

pub(super) fn equipment(cpu: &Cpu) -> u16 {
    cpu.read_u16(BDA, EQUIPMENT)
}

pub(super) fn memory_map(cpu: &mut Cpu) {
    const SIGNATURE: u32 = 0x534d_4150; // SMAP
    const DESCRIPTOR_SIZE: u32 = 20;
    const RAM: u32 = 1;
    const RESERVED: u32 = 2;
    let regions: [(u64, u64, u32); 3] = [
        (0, 0xa0000, RAM),
        (0xa0000, 0x60000, RESERVED),
        (0x100000, u64::from(cpu.extended_memory_kib()) * 1024, RAM),
    ];
    let number = cpu.get_reg(Register::EBX).unwrap() as usize;
    let offset = u32::from(cpu.regs.di);
    let region = regions.get(number).filter(|_| {
        cpu.profile() == CpuProfile::I386
            && cpu.get_reg(Register::EDX) == Some(SIGNATURE)
            && cpu.get_reg(Register::ECX).unwrap() >= DESCRIPTOR_SIZE
            && offset + DESCRIPTOR_SIZE <= 0x10000
    });
    let Some(&(base, length, kind)) = region else {
        cpu.regs.flags |= CARRY;
        cpu.regs.ax = (cpu.regs.ax & 0xff) | 0x8600;
        return;
    };
    let mut descriptor = [0; DESCRIPTOR_SIZE as usize];
    descriptor[..8].copy_from_slice(&base.to_le_bytes());
    descriptor[8..16].copy_from_slice(&length.to_le_bytes());
    descriptor[16..].copy_from_slice(&kind.to_le_bytes());
    cpu.load(cpu.regs.es, cpu.regs.di, &descriptor);

    let next = if number + 1 < regions.len() {
        number + 1
    } else {
        0
    };
    cpu.set_reg(Register::EAX, SIGNATURE).unwrap();
    cpu.set_reg(Register::EBX, next as u32).unwrap();
    cpu.set_reg(Register::ECX, DESCRIPTOR_SIZE).unwrap();
    cpu.regs.flags &= !CARRY;
}

pub(super) fn install_floppy(cpu: &mut Cpu) {
    cpu.write_u16(BDA, EQUIPMENT, equipment(cpu) | 1);
}

pub(super) fn signal_floppy_change(cpu: &mut Cpu) {
    cpu.write_u8(ROM, FLOPPY_CHANGE_LINE, 1);
}

pub(super) fn take_floppy_change(cpu: &mut Cpu) -> bool {
    // Keep the change line in firmware memory so it travels with saved guests.
    let changed = cpu.read_u8(ROM, FLOPPY_CHANGE_LINE) != 0;
    cpu.write_u8(ROM, FLOPPY_CHANGE_LINE, 0);
    changed
}

pub(super) fn default_keyboard(cpu: &Cpu) -> bool {
    let index = u16::from(KEYBOARD_VECTOR) * 4;
    cpu.read_u16(0, index) == KEYBOARD_ENTRY && cpu.read_u16(0, index + 2) == ROM
}

pub(super) fn hook(cpu: &Cpu) -> Option<Hook> {
    if cpu.regs.cs != ROM {
        return None;
    }
    match cpu.regs.ip {
        TIMER_ENTRY => Some(Hook::Tick),
        TIMER_RETURN => Some(Hook::TimerEoi),
        KEYBOARD_ENTRY => Some(Hook::Keyboard),
        _ => None,
    }
}

fn read_ticks(cpu: &Cpu) -> u32 {
    u32::from(cpu.read_u16(BDA, TICKS)) | (u32::from(cpu.read_u16(BDA, TICKS + 2)) << 16)
}

fn write_ticks(cpu: &mut Cpu, ticks: u32) {
    cpu.write_u16(BDA, TICKS, ticks as u16);
    cpu.write_u16(BDA, TICKS + 2, (ticks >> 16) as u16);
}

pub(super) fn advance_timer(cpu: &mut Cpu) {
    let mut ticks = read_ticks(cpu).wrapping_add(1);
    if ticks >= TICKS_PER_DAY {
        ticks = 0;
        cpu.write_u8(BDA, MIDNIGHT, cpu.read_u8(BDA, MIDNIGHT).wrapping_add(1));
    }
    write_ticks(cpu, ticks);
}

pub(super) fn take_timer(cpu: &mut Cpu) -> (u32, u8) {
    let midnight = cpu.read_u8(BDA, MIDNIGHT);
    cpu.write_u8(BDA, MIDNIGHT, 0);
    (read_ticks(cpu), midnight)
}

pub(super) fn set_timer(cpu: &mut Cpu, ticks: u32) {
    write_ticks(cpu, ticks);
    cpu.write_u8(BDA, MIDNIGHT, 0);
}

pub(super) fn bcd_pair(high: u8, low: u8) -> u16 {
    let encode = |value: u8| ((value / 10) << 4) | (value % 10);
    u16::from_be_bytes([encode(high), encode(low)])
}

pub(super) fn decode_bcd_pair(word: u16) -> Option<(u8, u8)> {
    let decode = |byte: u8| {
        let (tens, units) = (byte >> 4, byte & 0x0f);
        (tens <= 9 && units <= 9).then_some(tens * 10 + units)
    };
    let [high, low] = word.to_be_bytes();
    Some((decode(high)?, decode(low)?))
}

pub(super) fn rtc_dst(cpu: &Cpu) -> u8 {
    cpu.read_u8(ROM, RTC_DST_OPTION) & 1
}

pub(super) fn set_rtc_dst(cpu: &mut Cpu, option: u8) {
    cpu.write_u8(ROM, RTC_DST_OPTION, option);
}
