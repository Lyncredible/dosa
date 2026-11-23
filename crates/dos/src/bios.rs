//! Callable default BIOS interrupt handlers and the BIOS timer data area.

use dosa_cpu::Cpu;

use crate::keyboard::KEYBOARD_VECTOR;
use crate::pit::BIOS_DIVISOR;

pub(super) const TIMER_VECTOR: u8 = 0x08;
const USER_TIMER_VECTOR: u8 = 0x1c;
const ROM: u16 = 0xf000;
const TIMER_ENTRY: u16 = 0x0800;
const TIMER_RETURN: u16 = TIMER_ENTRY + 3;
const KEYBOARD_ENTRY: u16 = 0x0900;
const USER_TIMER_ENTRY: u16 = 0x1c00;
const BDA: u16 = 0x0040;
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
