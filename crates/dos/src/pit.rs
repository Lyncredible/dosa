use std::time::Duration;

pub(super) const BIOS_DIVISOR: u64 = 65_536;
pub(super) const FREQUENCY: u128 = 1_193_182;

pub(super) fn ticks(elapsed: Duration) -> u64 {
    (elapsed.as_nanos() * FREQUENCY / 1_000_000_000) as u64
}

pub(super) struct Pit {
    now: u64,
    counters: [Counter; 2],
    pending: bool,
    refresh: bool,
}

impl Pit {
    pub fn new(now: u64) -> Self {
        Self {
            now,
            counters: [Counter::new(now, BIOS_DIVISOR), Counter::new(now, 18)],
            pending: false,
            refresh: false,
        }
    }

    pub fn advance(&mut self, now: u64) {
        self.now = now;
        // The PIC retains one request even when several rising edges were missed.
        self.pending |= self.counters[0].advance(now) != 0;
        self.refresh ^= self.counters[1].advance(now) & 1 != 0;
    }

    pub fn irq_requested(&self) -> bool {
        self.pending
    }

    pub fn acknowledge_irq(&mut self) {
        self.pending = false;
    }

    pub fn refresh_toggle(&self) -> bool {
        self.refresh
    }

    pub fn port_read(&mut self, port: u16) -> Option<u8> {
        match port {
            0x40 | 0x41 => Some(self.counters[usize::from(port - 0x40)].read(self.now)),
            _ => None,
        }
    }

    pub fn port_write(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x40 | 0x41 => self.counters[usize::from(port - 0x40)].write(value, self.now),
            0x43 => match value >> 6 {
                index @ 0..=1 => self.counters[usize::from(index)].control(value, self.now),
                3 => {
                    for (index, counter) in self.counters.iter_mut().enumerate() {
                        if read_back_selects(value, index) {
                            counter.latch(value & 0x20 == 0, value & 0x10 == 0, self.now);
                        }
                    }
                }
                _ => (), // Channel 2 belongs to the speaker device.
            },
            _ => return false,
        }
        true
    }
}

struct Counter {
    control: u8,
    period: Option<Period>,
    reload: Option<Period>,
    next_edge: Option<u64>,
    low_byte: Option<u8>,
    read_high: bool,
    latched_count: Option<u16>,
    latched_status: Option<u8>,
}

impl Counter {
    fn new(now: u64, divisor: u64) -> Self {
        Self {
            control: 0x34,
            period: Some(Period::new(now, divisor)),
            reload: None,
            next_edge: Some(now + divisor),
            low_byte: None,
            read_high: false,
            latched_count: None,
            latched_status: None,
        }
    }

    fn advance(&mut self, now: u64) -> u64 {
        if let Some(reload) = self.reload.filter(|reload| reload.tick <= now) {
            let edges = self.advance_edges(reload.tick - 1);
            self.period = self.reload.take();
            if reload.starts_low {
                self.next_edge = Some(reload.tick + reload.divisor / 2);
                return edges + self.advance_edges(now);
            }
            self.next_edge = Some(reload.tick + reload.divisor);
            return edges + 1 + self.advance_edges(now);
        }
        self.advance_edges(now)
    }

    fn advance_edges(&mut self, now: u64) -> u64 {
        let Some(edge) = self.next_edge.filter(|&edge| now >= edge) else {
            return 0;
        };
        if matches!(self.mode(), 2 | 3) {
            let divisor = self
                .period
                .expect("scheduled periodic edge has a count")
                .divisor;
            let count = (now - edge) / divisor + 1;
            self.next_edge = Some(now + divisor - (now - edge) % divisor);
            count
        } else {
            self.next_edge = None;
            1
        }
    }

    fn control(&mut self, value: u8, now: u64) {
        if value & 0x30 == 0 {
            self.latch(true, false, now);
            return;
        }
        self.control = value & 0x3f;
        self.period = None;
        self.reload = None;
        self.next_edge = None;
        self.low_byte = None;
        self.read_high = false;
        self.latched_count = None;
        self.latched_status = None;
    }

    fn latch(&mut self, count: bool, status: bool, now: u64) {
        if count && self.latched_count.is_none() {
            self.latched_count = Some(self.count(now));
            self.read_high = false;
        }
        if status && self.latched_status.is_none() {
            let null_count =
                self.reload.is_some() || self.period.is_none_or(|period| period.tick > now);
            self.latched_status =
                Some(self.control | u8::from(self.output(now)) << 7 | u8::from(null_count) << 6);
        }
    }

    fn read(&mut self, now: u64) -> u8 {
        if let Some(status) = self.latched_status.take() {
            return status;
        }
        let [low, high] = self
            .latched_count
            .unwrap_or_else(|| self.count(now))
            .to_le_bytes();
        let byte = match self.control & 0x30 {
            0x10 => low,
            0x20 => high,
            _ if self.read_high => {
                self.read_high = false;
                high
            }
            _ => {
                self.read_high = true;
                return low;
            }
        };
        self.latched_count = None;
        byte
    }

    fn write(&mut self, value: u8, now: u64) {
        let raw = match self.control & 0x30 {
            0x10 => u16::from(value),
            0x20 => u16::from(value) << 8,
            _ => match self.low_byte.take() {
                Some(low) => u16::from_le_bytes([low, value]),
                None => {
                    self.low_byte = Some(value);
                    if self.mode() == 0 {
                        self.period = None;
                        self.next_edge = None;
                    }
                    return;
                }
            },
        };
        let divisor = decode_count(raw, self.control & 1 != 0);
        let mode = self.mode();
        if let Some(reload) = self
            .period
            .and_then(|period| period.deferred_reload(mode, now, divisor))
        {
            self.reload = Some(reload);
            return;
        }
        // The board wires both gates high. Hardware-triggered modes require
        // a new rising gate edge and cannot start from a counter write.
        if !matches!(mode, 1 | 5) {
            let tick = now + 1;
            self.period = Some(Period::new(tick, divisor));
            self.next_edge = Some(tick + divisor + u64::from(mode == 4));
        }
    }

    fn mode(&self) -> u8 {
        match (self.control >> 1) & 7 {
            6 => 2,
            7 => 3,
            mode => mode,
        }
    }

    fn count(&self, now: u64) -> u16 {
        let Some(period) = self.period else {
            return 0;
        };
        let elapsed = now.saturating_sub(period.tick);
        let divisor = period.divisor;
        let limit = if self.control & 1 == 0 {
            BIOS_DIVISOR
        } else {
            10_000
        };
        let value = match self.mode() {
            2 => divisor - elapsed % divisor,
            3 => {
                let phase = period.phase(now);
                let half = divisor.div_ceil(2);
                let steps = if phase < half { phase } else { phase - half };
                (divisor & !1).saturating_sub(steps * 2)
            }
            _ => (divisor + limit - elapsed % limit) % limit,
        };
        if self.control & 1 == 0 {
            value as u16
        } else {
            encode_bcd(value)
        }
    }

    fn output(&self, now: u64) -> bool {
        let Some(period) = self.period.filter(|period| period.tick <= now) else {
            return self.mode() != 0;
        };
        let elapsed = now - period.tick;
        let divisor = period.divisor;
        match self.mode() {
            0 => elapsed >= divisor,
            2 => elapsed % divisor != divisor - 1,
            3 => period.phase(now) < divisor.div_ceil(2),
            4 => elapsed != divisor,
            _ => true,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Period {
    pub tick: u64,
    pub divisor: u64,
    starts_low: bool,
}

impl Period {
    pub fn new(tick: u64, divisor: u64) -> Self {
        Self {
            tick,
            divisor,
            starts_low: false,
        }
    }

    pub fn phase(self, now: u64) -> u64 {
        let elapsed = now.saturating_sub(self.tick) % self.divisor;
        let offset = u64::from(self.starts_low) * self.divisor.div_ceil(2);
        (elapsed + offset) % self.divisor
    }

    pub fn deferred_reload(self, mode: u8, now: u64, divisor: u64) -> Option<Self> {
        if !matches!(mode, 2 | 3) || self.tick > now {
            return None;
        }
        let phase = self.phase(now);
        let half = self.divisor.div_ceil(2);
        let starts_low = mode == 3 && phase < half;
        let boundary = if starts_low { half } else { self.divisor };
        Some(Self {
            tick: now + boundary - phase,
            divisor,
            starts_low,
        })
    }
}

pub(super) fn read_back_selects(control: u8, channel: usize) -> bool {
    control & 0xc0 == 0xc0 && control & (2 << channel) != 0
}

pub(super) fn decode_count(raw: u16, bcd: bool) -> u64 {
    if !bcd {
        return if raw == 0 {
            BIOS_DIVISOR
        } else {
            u64::from(raw)
        };
    }
    let count = (0..4).rev().fold(0, |count, digit| {
        count * 10 + u64::from((raw >> (digit * 4)) & 0xf)
    }) % 10_000;
    if count == 0 { 10_000 } else { count }
}

pub(super) fn encode_bcd(mut value: u64) -> u16 {
    let mut packed = 0;
    for digit in 0..4 {
        packed |= ((value % 10) as u16) << (digit * 4);
        value /= 10;
    }
    packed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_zero_reloads_divisor_and_coalesces_missed_edges() {
        let mut pit = Pit::new(100);
        pit.advance(100 + BIOS_DIVISOR - 1);
        assert!(!pit.irq_requested());
        pit.advance(100 + BIOS_DIVISOR);
        assert!(pit.irq_requested());
        pit.acknowledge_irq();

        pit.port_write(0x43, 0x36);
        pit.port_write(0x40, 0x10);
        pit.port_write(0x40, 0x00);
        pit.port_write(0x43, 0xb6); // channel 2 does not change channel 0
        pit.advance(100 + BIOS_DIVISOR + 15);
        assert!(!pit.irq_requested());
        pit.advance(100 + BIOS_DIVISOR + 49);
        assert!(pit.irq_requested());
        pit.acknowledge_irq();
        pit.advance(100 + BIOS_DIVISOR + 63);
        assert!(!pit.irq_requested());
        pit.advance(100 + BIOS_DIVISOR + 65);
        assert!(pit.irq_requested());
    }

    #[test]
    fn latch_preserves_count_between_reads() {
        let mut pit = Pit::new(0);
        pit.advance(1);
        pit.port_write(0x43, 0);
        assert_eq!(pit.port_read(0x40), Some(0xff));
        pit.advance(0x1000);
        assert_eq!(pit.port_read(0x40), Some(0xff));
    }

    #[test]
    fn mode_two_reload_preserves_old_irq_boundary_and_clears_null_count() {
        let mut pit = Pit::new(0);
        pit.port_write(0x43, 0x14);
        pit.port_write(0x40, 10);
        pit.advance(4);
        pit.port_write(0x40, 4);
        pit.port_write(0x43, 0xc2);
        assert_eq!(pit.port_read(0x40), Some(0xd4));
        assert_eq!(pit.port_read(0x40), Some(7));
        pit.advance(10);
        assert!(!pit.irq_requested());
        pit.advance(11);
        assert!(pit.irq_requested());
        pit.acknowledge_irq();
        pit.port_write(0x43, 0xc2);
        assert_eq!(pit.port_read(0x40), Some(0x94));
        assert_eq!(pit.port_read(0x40), Some(4));
        pit.advance(14);
        assert!(!pit.irq_requested());
        pit.advance(15);
        assert!(pit.irq_requested());
    }

    #[test]
    fn mode_three_refresh_counts_rises_across_a_deferred_half_cycle_reload() {
        let mut pit = Pit::new(0);
        pit.port_write(0x43, 0x56);
        pit.port_write(0x41, 10);
        pit.advance(4);
        pit.port_write(0x41, 7);
        pit.port_write(0x41, 9); // Last completed count wins.
        pit.advance(6);
        assert!(!pit.refresh_toggle()); // Load 9 at the falling edge.
        assert_eq!(pit.port_read(0x41), Some(8));
        pit.advance(9);
        assert!(!pit.refresh_toggle());
        pit.advance(19); // Rising edges at 10 and 19.
        assert!(!pit.refresh_toggle());
        pit.advance(28);
        assert!(pit.refresh_toggle());
    }

    #[test]
    fn mode_zero_raises_irq_once_after_the_next_clock_loads_the_count() {
        let mut pit = Pit::new(0);
        pit.port_write(0x43, 0x30);
        pit.port_write(0x40, 4);
        pit.port_write(0x40, 0);
        pit.advance(4);
        assert!(!pit.irq_requested());
        pit.advance(5);
        assert!(pit.irq_requested());
        pit.acknowledge_irq();
        pit.advance(1000);
        assert!(!pit.irq_requested());
    }

    #[test]
    fn refresh_and_bcd_readback_track_channel_one_independently() {
        let mut pit = Pit::new(0);
        pit.advance(17);
        assert!(!pit.refresh_toggle());
        pit.advance(18);
        assert!(pit.refresh_toggle());
        pit.advance(54);
        assert!(pit.refresh_toggle());
        pit.port_write(0x43, 0x75); // channel 1, both bytes, mode 2, BCD
        pit.port_write(0x43, 0xe4); // latch status for channel 1 only
        assert_eq!(pit.port_read(0x41), Some(0xf5));
        pit.port_write(0x41, 0x34);
        pit.port_write(0x41, 0x12);
        pit.advance(60); // five ticks after the load
        pit.port_write(0x43, 0xc4); // latch channel 1 status and count
        pit.advance(62);
        assert_eq!(pit.port_read(0x41), Some(0xb5));
        assert_eq!(pit.port_read(0x41), Some(0x29));
        assert_eq!(pit.port_read(0x41), Some(0x12));
        pit.advance(1288);
        assert!(pit.refresh_toggle());
        pit.advance(1289);
        assert!(!pit.refresh_toggle());
        assert!(!pit.irq_requested());
    }

    #[test]
    fn guest_reads_the_refresh_toggle_through_port_61() {
        let mut machine = crate::Machine::new();
        machine.cpu.regs.cs = 0x1000;
        machine.cpu.load(
            0x1000,
            0,
            &[
                0xe4, 0x61, 0xc6, 0xc0, 0xff, 0xe6, 0x61, 0xe4, 0x61, 0xe4, 0x61,
            ],
        );
        machine.step().unwrap();
        assert_eq!(machine.cpu.regs.ax & 0x10, 0);
        machine.step().unwrap();
        machine.step().unwrap();
        machine.step().unwrap();
        assert_eq!(machine.cpu.regs.ax & 0x10, 0);
        machine.pit.advance(18);
        machine.step().unwrap();
        assert_eq!(machine.cpu.regs.ax & 0x10, 0x10);
    }
}
