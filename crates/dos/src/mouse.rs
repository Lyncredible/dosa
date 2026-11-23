//! Integrated mouse services for DOS guests using the INT 33h driver ABI.

use dosa_cpu::{Cpu, RegisterSnapshot, Registers};

const VECTOR: u8 = 0x33;
const ROM_SEGMENT: u16 = 0xf000;
const DRIVER_ENTRY: u16 = 0x3300;
const CALLBACK_RETURN: u16 = 0x3302;
const BUTTONS: usize = 3;
const NORMALIZED_MAX: i32 = u16::MAX as i32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseCursor {
    /// Position as a fraction of viewport width and height, from 0 to 65535.
    pub x: u16,
    pub y: u16,
    pub hotspot_x: i16,
    pub hotspot_y: i16,
    /// For each row, bit 15 is the leftmost pixel.
    pub screen_mask: [u16; 16],
    pub cursor_mask: [u16; 16],
}

#[derive(Clone, Copy)]
struct Axis {
    low: i32,
    high: i32,
    position: i32,
}

impl Axis {
    fn new(high: u16) -> Self {
        Self {
            low: 0,
            high: i32::from(high),
            position: i32::from(high) / 2,
        }
    }

    fn set_position(&mut self, position: i32) {
        self.position = position.clamp(self.low, self.high);
    }

    fn translate(&self, normalized: u16) -> i32 {
        self.low
            + ((i64::from(normalized) * i64::from(self.high - self.low)
                + i64::from(NORMALIZED_MAX / 2))
                / i64::from(NORMALIZED_MAX)) as i32
    }

    fn normalized(&self) -> u16 {
        let extent = self.high - self.low;
        if extent == 0 {
            return 0;
        }
        (i64::from(self.position - self.low) * i64::from(NORMALIZED_MAX) / i64::from(extent)) as u16
    }

    fn constrain(&mut self, low: i16, high: i16) {
        self.low = i32::from(low.min(high));
        self.high = i32::from(low.max(high));
        self.set_position(self.position);
    }

    fn register(self) -> u16 {
        self.position as u16
    }
}

#[derive(Clone, Copy, Default)]
struct ButtonEvent {
    count: u16,
    x: u16,
    y: u16,
}

#[derive(Clone, Copy, Default)]
struct Callback {
    mask: u16,
    segment: u16,
    offset: u16,
}

pub(crate) struct Mouse {
    active: bool,
    display_depth: i16,
    x: Axis,
    y: Axis,
    normalized: (u16, u16),
    buttons: u8,
    presses: [ButtonEvent; BUTTONS],
    releases: [ButtonEvent; BUTTONS],
    motion: [i16; 2],
    callback_motion: [i16; 2],
    sensitivity: [u16; 3],
    ratio: [u16; 2],
    cursor: MouseCursor,
    hidden_region: Option<[u16; 4]>,
    callback: Callback,
    event_mask: u16,
    saved_registers: Option<RegisterSnapshot>,
}

impl Default for Mouse {
    fn default() -> Self {
        Self {
            active: false,
            display_depth: -1,
            x: Axis::new(639),
            y: Axis::new(199),
            normalized: (32768, 32768),
            buttons: 0,
            presses: [ButtonEvent::default(); BUTTONS],
            releases: [ButtonEvent::default(); BUTTONS],
            motion: [0; 2],
            callback_motion: [0; 2],
            sensitivity: [50; 3],
            ratio: [8, 16],
            cursor: default_cursor(),
            hidden_region: None,
            callback: Callback::default(),
            event_mask: 0,
            saved_registers: None,
        }
    }
}

impl Mouse {
    pub(crate) fn install(cpu: &mut Cpu) {
        cpu.write_u16(0, u16::from(VECTOR) * 4, DRIVER_ENTRY);
        cpu.write_u16(0, u16::from(VECTOR) * 4 + 2, ROM_SEGMENT);
        cpu.write_u8(ROM_SEGMENT, DRIVER_ENTRY, 0xcf); // IRET for callers chaining via PUSHF/CALL FAR.
    }

    pub(crate) fn at_driver_entry(&self, cpu: &Cpu) -> bool {
        cpu.regs.cs == ROM_SEGMENT && cpu.regs.ip == DRIVER_ENTRY
    }

    pub(crate) fn owns_vector(&self, cpu: &Cpu, vector: u8) -> bool {
        vector == VECTOR
            && cpu.read_u16(0, u16::from(VECTOR) * 4) == DRIVER_ENTRY
            && cpu.read_u16(0, u16::from(VECTOR) * 4 + 2) == ROM_SEGMENT
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn cursor(&self) -> Option<MouseCursor> {
        let position = (self.x.register(), self.y.register());
        let blocked = self
            .hidden_region
            .is_some_and(|[left, top, right, bottom]| {
                (left..=right).contains(&position.0) && (top..=bottom).contains(&position.1)
            });
        (self.active && self.display_depth >= 0 && !blocked).then(|| MouseCursor {
            x: self.x.normalized(),
            y: self.y.normalized(),
            ..self.cursor
        })
    }

    pub(crate) fn move_to(&mut self, x: u16, y: u16) {
        self.normalized = (x, y);
        let new_x = self.x.translate(x);
        let new_y = self.y.translate(y);
        let delta = [new_x - self.x.position, new_y - self.y.position];
        self.x.set_position(new_x);
        self.y.set_position(new_y);
        if delta == [0, 0] {
            return;
        }
        for (axis, pixels) in delta.into_iter().enumerate() {
            let motion = (i64::from(pixels) * i64::from(self.ratio[axis]) / 8)
                .clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16;
            self.motion[axis] = self.motion[axis].saturating_add(motion);
            if self.callback.mask & 1 != 0 {
                self.callback_motion[axis] = self.callback_motion[axis].saturating_add(motion);
            }
        }
        self.event_mask |= self.callback.mask & 1;
    }

    pub(crate) fn buttons(&mut self, mask: u8) {
        let mask = mask & 0b111;
        let changed = mask ^ self.buttons;
        self.buttons = mask;
        for button in 0..BUTTONS {
            let bit = 1 << button;
            if changed & bit == 0 {
                continue;
            }
            let down = mask & bit != 0;
            let (data, event) = if down {
                (&mut self.presses, 1 << (button * 2 + 1))
            } else {
                (&mut self.releases, 1 << (button * 2 + 2))
            };
            let record = &mut data[button];
            record.count = record.count.saturating_add(1);
            record.x = self.x.register();
            record.y = self.y.register();
            self.event_mask |= self.callback.mask & event;
        }
    }

    pub(crate) fn dispatch_callback(&mut self, cpu: &mut Cpu, interrupt_enabled: bool) {
        if let Some(saved) = self.saved_registers {
            if cpu.regs.cs == ROM_SEGMENT && cpu.regs.ip == CALLBACK_RETURN {
                cpu.restore_registers(saved);
                self.saved_registers = None;
            } else {
                return;
            }
        }
        if !interrupt_enabled || self.event_mask == 0 {
            return;
        }
        let callback = self.callback;
        let events = std::mem::take(&mut self.event_mask);
        let [horizontal, vertical] = std::mem::take(&mut self.callback_motion);
        self.saved_registers = Some(cpu.save_registers());
        cpu.regs.sp = cpu.regs.sp.wrapping_sub(2);
        cpu.write_u16(cpu.regs.ss, cpu.regs.sp, ROM_SEGMENT);
        cpu.regs.sp = cpu.regs.sp.wrapping_sub(2);
        cpu.write_u16(cpu.regs.ss, cpu.regs.sp, CALLBACK_RETURN);
        cpu.regs.cs = callback.segment;
        cpu.regs.ip = callback.offset;
        cpu.regs.ax = events;
        cpu.regs.bx = u16::from(self.buttons);
        cpu.regs.cx = self.x.register();
        cpu.regs.dx = self.y.register();
        cpu.regs.si = horizontal as u16;
        cpu.regs.di = vertical as u16;
    }

    pub(crate) fn interrupt(&mut self, cpu: &mut Cpu, (width, height): (u16, u16)) {
        let Registers {
            ax, bx, cx, dx, es, ..
        } = cpu.regs;
        self.active = true;
        match ax {
            0 | 0x21 => {
                let host = self.normalized;
                let suspended = self.saved_registers;
                *self = Self::default();
                self.active = true;
                self.saved_registers = suspended;
                self.x = Axis::new(width.saturating_sub(1));
                self.y = Axis::new(height.saturating_sub(1));
                self.move_to(host.0, host.1);
                self.motion = [0; 2];
                cpu.regs.ax = 0xffff;
                cpu.regs.bx = BUTTONS as u16;
            }
            1 => {
                self.display_depth = self.display_depth.saturating_add(1).min(0);
                self.hidden_region = None;
            }
            2 => self.display_depth = self.display_depth.saturating_sub(1),
            3 => {
                cpu.regs.bx = u16::from(self.buttons);
                cpu.regs.cx = self.x.register();
                cpu.regs.dx = self.y.register();
            }
            4 => {
                self.x.set_position(i32::from(cx as i16));
                self.y.set_position(i32::from(dx as i16));
            }
            5 | 6 => {
                cpu.regs.ax = u16::from(self.buttons);
                cpu.regs.bx = 0;
                cpu.regs.cx = 0;
                cpu.regs.dx = 0;
                let events = if ax == 5 {
                    &mut self.presses
                } else {
                    &mut self.releases
                };
                if let Some(record) = events.get_mut(usize::from(bx)) {
                    cpu.regs.bx = std::mem::take(&mut record.count);
                    cpu.regs.cx = record.x;
                    cpu.regs.dx = record.y;
                }
            }
            7 => self.x.constrain(cx as i16, dx as i16),
            8 => self.y.constrain(cx as i16, dx as i16),
            9 => {
                self.cursor.hotspot_x = bx as i16;
                self.cursor.hotspot_y = cx as i16;
                for row in 0..16 {
                    self.cursor.screen_mask[row] =
                        cpu.read_u16(es, dx.wrapping_add((row * 2) as u16));
                    self.cursor.cursor_mask[row] =
                        cpu.read_u16(es, dx.wrapping_add(32 + (row * 2) as u16));
                }
            }
            0x0b => {
                let [horizontal, vertical] = std::mem::take(&mut self.motion);
                cpu.regs.cx = horizontal as u16;
                cpu.regs.dx = vertical as u16;
            }
            0x0c | 0x14 => {
                if ax == 0x14 {
                    cpu.regs.cx = self.callback.mask;
                    cpu.regs.es = self.callback.segment;
                    cpu.regs.dx = self.callback.offset;
                }
                self.callback = Callback {
                    mask: if es == 0 && dx == 0 { 0 } else { cx & 0x7f },
                    segment: es,
                    offset: dx,
                };
                self.event_mask = 0;
                self.callback_motion = [0; 2];
            }
            0x0f => self.ratio = [cx.max(1), dx.max(1)],
            0x10 => self.hidden_region = Some([cx, dx, cpu.regs.si, cpu.regs.di]),
            0x13 => self.sensitivity[2] = dx,
            0x1a => self.sensitivity = [bx, cx, dx],
            0x1b => [cpu.regs.bx, cpu.regs.cx, cpu.regs.dx] = self.sensitivity,
            0x24 => {
                cpu.regs.bx = 0x0600; // Driver interface version.
                cpu.regs.cx = 0x0400; // Integrated PS/2-style pointing device, no reported IRQ.
            }
            0x26 => {
                cpu.regs.bx = 0;
                cpu.regs.cx = self.x.high as u16;
                cpu.regs.dx = self.y.high as u16;
            }
            _ => {}
        }
    }
}

fn default_cursor() -> MouseCursor {
    let mut screen_mask = [0xffff; 16];
    let mut cursor_mask = [0; 16];
    for (row, (screen, cursor)) in screen_mask
        .iter_mut()
        .zip(&mut cursor_mask)
        .enumerate()
        .take(12)
    {
        let shape = if row < 9 {
            0xffff << (15 - row)
        } else {
            0x0e00
        };
        *screen = !shape;
        if (2..9).contains(&row) {
            *cursor = shape & 0x3fff & !(1 << (15 - row));
        } else if row >= 9 {
            *cursor = 0x0400;
        }
    }
    MouseCursor {
        x: 0,
        y: 0,
        hotspot_x: 0,
        hotspot_y: 0,
        screen_mask,
        cursor_mask,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Machine, RunState};

    #[test]
    fn dos_services_report_viewport_motion_buttons_and_cursor_visibility() {
        let mut machine = Machine::new();
        machine.state = RunState::Running;
        machine.interrupt(0x33).unwrap();
        assert_eq!((machine.cpu.regs.ax, machine.cpu.regs.bx), (0xffff, 3));
        assert!(machine.mouse_active());
        assert!(machine.mouse_cursor().is_none());

        machine.cpu.regs.ax = 7;
        machine.cpu.regs.cx = 50;
        machine.cpu.regs.dx = 350;
        machine.interrupt(0x33).unwrap();
        machine.mouse_move(65535, 0);
        machine.mouse_buttons(0b001);
        machine.mouse_buttons(0);
        machine.cpu.regs.ax = 5;
        machine.cpu.regs.bx = 0;
        machine.interrupt(0x33).unwrap();
        assert_eq!((machine.cpu.regs.ax, machine.cpu.regs.bx), (0, 1));
        assert_eq!((machine.cpu.regs.cx, machine.cpu.regs.dx), (350, 0));

        machine.cpu.regs.ax = 6;
        machine.cpu.regs.bx = 0;
        machine.interrupt(0x33).unwrap();
        assert_eq!(machine.cpu.regs.bx, 1);
        machine.cpu.regs.ax = 1;
        machine.interrupt(0x33).unwrap();
        let cursor = machine.mouse_cursor().unwrap();
        assert_eq!((cursor.x, cursor.y), (65535, 0));
        machine.cpu.regs.ax = 2;
        machine.interrupt(0x33).unwrap();
        assert!(machine.mouse_cursor().is_none());
    }

    #[test]
    fn mouse_callback_returns_with_original_guest_registers() {
        let mut machine = Machine::new();
        let cpu = &mut machine.cpu;
        cpu.load(0x100, 0x0002, &[0x90]); // NOP after callback
        cpu.load(
            0x300,
            0x0010,
            &[
                0xa3, 0x00, 0x05, // MOV [0500h], AX
                0x66, 0xb8, 0x78, 0x56, 0x34, 0x12, // MOV EAX, 12345678h
                0x8e, 0xe0, // MOV FS, AX
                0xcb, // RETF
            ],
        );
        cpu.regs.cs = 0x100;
        cpu.regs.ip = 2;
        cpu.regs.ss = 0x200;
        cpu.regs.sp = 0xfffe;
        cpu.regs.ds = 0x200;
        cpu.regs.ax = 0x0c;
        cpu.regs.cx = 3;
        cpu.regs.es = 0x300;
        cpu.regs.dx = 0x10;
        cpu.regs.flags = 0x0202;
        machine.interrupt(0x33).unwrap();
        let original = machine.cpu.save_registers();
        machine.mouse_move(65535, 65535);
        machine.mouse_buttons(1);
        for _ in 0..5 {
            machine.step().unwrap();
        }
        assert_eq!(machine.cpu.read_u16(0x200, 0x500), 3);
        assert_eq!(machine.cpu.regs.ip, 3);
        machine.cpu.regs.ip = 2;
        assert_eq!(machine.cpu.save_registers(), original);
    }
}
