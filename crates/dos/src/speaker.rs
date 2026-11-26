//! PIT channel 2 and the PC speaker gate.

use crate::pit::{BIOS_DIVISOR, Period, decode_count, encode_bcd, read_back_selects};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Speaker {
    now: u64,
    period: Option<Period>,
    reload: Option<Period>,
    control: u8,
    low: Option<u8>,
    read_high: bool,
    latched: Option<u16>,
    latched_status: Option<u8>,
    gate: u8,
}

impl Speaker {
    pub(super) fn valid_snapshot(&self, elapsed: std::time::Duration) -> bool {
        let now = crate::pit::ticks(elapsed);
        let horizon = now.saturating_add(BIOS_DIVISOR + 1);
        self.now <= now
            && self
                .period
                .is_none_or(|period| period.valid_snapshot(horizon))
            && self
                .reload
                .is_none_or(|period| period.valid_snapshot(horizon))
    }

    pub fn new(now: u64) -> Self {
        Self {
            now,
            period: Some(Period::new(now, BIOS_DIVISOR)),
            reload: None,
            control: 0x36,
            low: None,
            read_high: false,
            latched: None,
            latched_status: None,
            gate: 0,
        }
    }

    pub fn advance(&mut self, now: u64) {
        self.now = now;
        if self.reload.is_some_and(|reload| reload.tick <= now) {
            self.period = self.reload.take();
        }
    }

    fn period_at(&self, tick: u64) -> Option<Period> {
        self.reload
            .filter(|reload| reload.tick <= tick)
            .or(self.period)
            .filter(|period| period.tick <= tick)
    }

    pub fn sample(&self, tick: u64) -> f32 {
        if self.gate & 2 == 0 {
            0.0
        } else if self.output(tick) {
            0.12
        } else {
            -0.12
        }
    }

    fn output(&self, now: u64) -> bool {
        if self.gate & 1 == 0 {
            return true;
        }
        let mode = self.mode();
        let Some(period) = self.period_at(now) else {
            return mode != 0;
        };
        let elapsed = now - period.tick;
        let divisor = period.divisor;
        match mode {
            0 | 1 => elapsed >= divisor,
            2 => elapsed % divisor != divisor - 1,
            3 => period.phase(now) < divisor.div_ceil(2),
            4 | 5 => elapsed != divisor,
            _ => true,
        }
    }

    fn count(&self) -> u16 {
        let Some(period) = self.period_at(self.now) else {
            return 0;
        };
        let elapsed = self.now - period.tick;
        let divisor = period.divisor;
        let remaining = match self.mode() {
            2 => divisor - elapsed % divisor,
            3 => {
                let phase = period.phase(self.now);
                if phase < divisor.div_ceil(2) {
                    if phase == 0 {
                        divisor
                    } else {
                        (divisor + (divisor & 1)).saturating_sub(phase * 2)
                    }
                } else {
                    (divisor & !1).saturating_sub((phase - divisor.div_ceil(2)) * 2)
                }
            }
            _ => divisor.saturating_sub(elapsed),
        };
        if self.control & 1 != 0 {
            encode_bcd(remaining)
        } else {
            remaining as u16
        }
    }

    fn mode(&self) -> u8 {
        let mode = self.control >> 1 & 7;
        if mode >= 6 { mode - 4 } else { mode }
    }

    fn latch_count(&mut self) {
        if self.latched.is_none() {
            self.latched = Some(self.count());
            self.read_high = false;
        }
    }

    pub fn read_back(&mut self, value: u8) {
        if !read_back_selects(value, 2) {
            return;
        }
        if value & 0x20 == 0 {
            self.latch_count();
        }
        if value & 0x10 == 0 && self.latched_status.is_none() {
            let null_count = self.reload.is_some() || self.period_at(self.now).is_none();
            self.latched_status = Some(
                self.control | u8::from(self.output(self.now)) << 7 | u8::from(null_count) << 6,
            );
        }
    }

    pub fn read_port(&mut self, port: u16) -> Option<u8> {
        match port {
            0x61 => Some((self.gate & !0x20) | (u8::from(self.output(self.now)) << 5)),
            0x42 => {
                if let Some(status) = self.latched_status.take() {
                    return Some(status);
                }
                let [low, high] = self.latched.unwrap_or_else(|| self.count()).to_le_bytes();
                let value = match self.control & 0x30 {
                    0x10 => low,
                    0x20 => high,
                    _ if self.read_high => {
                        self.read_high = false;
                        high
                    }
                    _ => {
                        self.read_high = true;
                        return Some(low);
                    }
                };
                self.latched = None;
                Some(value)
            }
            _ => None,
        }
    }

    pub fn write_port(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x61 => {
                if self.gate & 1 == 0 && value & 1 != 0 {
                    self.period = self
                        .reload
                        .take()
                        .or(self.period)
                        .map(|period| Period::new(self.now + 1, period.divisor));
                }
                self.gate = value;
            }
            0x43 if value & 0xc0 == 0x80 => {
                if value & 0x30 == 0 {
                    self.latch_count();
                } else {
                    self.control = value & 0x3f;
                    self.period = None;
                    self.reload = None;
                    self.low = None;
                    self.read_high = false;
                    self.latched = None;
                    self.latched_status = None;
                }
            }
            0x42 => {
                let raw = match self.control & 0x30 {
                    0x10 => u16::from(value),
                    0x20 => u16::from(value) << 8,
                    _ => match self.low.take() {
                        Some(low) => u16::from_le_bytes([low, value]),
                        None => {
                            self.low = Some(value);
                            return true;
                        }
                    },
                };
                let divisor = decode_count(raw, self.control & 1 != 0);
                if self.gate & 1 != 0 {
                    self.reload = self
                        .period
                        .and_then(|period| period.deferred_reload(self.mode(), self.now, divisor));
                    if self.reload.is_some() {
                        return true;
                    }
                }
                self.reload = None;
                self.period = Some(Period::new(self.now + 1, divisor));
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_mode_three_divisors_have_the_longer_high_phase() {
        let mut speaker = Speaker::new(0);
        speaker.write_port(0x43, 0xb6);
        speaker.write_port(0x42, 101);
        speaker.write_port(0x42, 0);
        speaker.write_port(0x61, 3);
        speaker.advance(2);
        assert_eq!(speaker.count(), 100);
        assert!(speaker.output(51));
        assert!(!speaker.output(52));
        speaker.advance(52);
        assert_eq!(speaker.count(), 100);
        assert!(speaker.output(102));
    }

    #[test]
    fn mode_two_has_a_single_clock_low_pulse_and_latched_reload() {
        let mut speaker = Speaker::new(0);
        speaker.write_port(0x43, 0xb4);
        speaker.write_port(0x42, 10);
        speaker.write_port(0x42, 0);
        speaker.write_port(0x61, 3);
        assert!(speaker.output(9));
        assert!(!speaker.output(10));
        assert!(speaker.output(11));
        speaker.advance(10);
        speaker.write_port(0x43, 0x80);
        assert_eq!(speaker.read_port(0x42), Some(1));
        speaker.advance(11);
        assert_eq!(speaker.read_port(0x42), Some(0));
        assert_eq!(speaker.count(), 10);
    }

    #[test]
    fn dos_port_instructions_program_the_speaker() {
        let mut machine = crate::Machine::new();
        machine.cpu.regs.cs = 0x1000;
        machine.cpu.regs.ip = 0;
        machine.cpu.load(
            0x1000,
            0,
            &[
                0xba, 0x43, 0, 0xb0, 0xb6, 0xee, 0xba, 0x42, 0, 0xb0, 100, 0xee, 0xb0, 0, 0xee,
                0xba, 0x61, 0, 0xb0, 3, 0xee, 0xeb, 0xfe,
            ],
        );
        machine.state = crate::RunState::Running;
        machine.run(20).unwrap();
        machine
            .audio
            .advance(std::time::Duration::from_secs(1), &machine.cpu);
        let samples = machine.take_audio();
        assert!(samples.contains(&0.12) && samples.contains(&-0.12));
    }

    #[test]
    fn tone_frequency_gate_and_latch_are_sampled() {
        let mut speaker = Speaker::new(0);
        speaker.write_port(0x43, 0xb6);
        speaker.write_port(0x42, 100);
        speaker.write_port(0x42, 0);
        speaker.write_port(0x61, 3);
        assert!(speaker.output(50));
        assert!(!speaker.output(51));
        assert!(speaker.output(101));
        speaker.advance(11);
        speaker.write_port(0x43, 0x80);
        assert_eq!(speaker.read_port(0x42), Some(80));
        speaker.advance(30);
        assert_eq!(speaker.read_port(0x42), Some(0));
        assert_eq!(speaker.sample(50), 0.12);
        assert_eq!(speaker.sample(51), -0.12);
        speaker.write_port(0x61, 0);
        assert_eq!(speaker.sample(100), 0.0);
    }

    #[test]
    fn bcd_read_back_latches_status_before_count_and_keeps_unread_values() {
        let mut speaker = Speaker::new(0);
        speaker.write_port(0x43, 0xb5);
        speaker.write_port(0x61, 3);
        speaker.write_port(0x42, 0x34);
        speaker.read_back(0xe8);
        assert_eq!(speaker.read_port(0x42), Some(0xf5));
        speaker.write_port(0x42, 0x12);
        speaker.read_back(0xe8);
        assert_eq!(speaker.read_port(0x42), Some(0xf5));

        speaker.advance(6);
        speaker.read_back(0xc8);
        speaker.advance(1234);
        assert!(!speaker.output(1234));
        speaker.read_back(0xc8);
        assert_eq!(speaker.read_port(0x42), Some(0xb5));
        assert_eq!(speaker.read_port(0x42), Some(0x29));
        assert_eq!(speaker.read_port(0x42), Some(0x12));
        assert!(speaker.output(1235));

        speaker.write_port(0x43, 0x95); // LSB-only BCD, zero means 10,000
        speaker.write_port(0x42, 0);
        speaker.advance(1235);
        assert_eq!(speaker.read_port(0x42), Some(0));
        assert!(!speaker.output(11234));
        assert!(speaker.output(11235));
    }

    #[test]
    fn guest_can_latch_status_for_both_system_timer_and_speaker() {
        let mut machine = crate::Machine::new();
        machine.cpu.regs.cs = 0x1000;
        machine.cpu.load(
            0x1000,
            0,
            &[
                0xb0, 0xb5, 0xe6, 0x43, // Program channel 2 without loading a count
                0xb0, 0xea, 0xe6, 0x43, // Latch status for channels 0 and 2
                0xe4, 0x42, 0x88, 0xc3, // Save channel 2 status in BL
                0xe4, 0x40, // Read channel 0 status into AL
            ],
        );
        for _ in 0..7 {
            machine.step().unwrap();
        }
        assert_eq!(machine.cpu.regs.ax as u8 & 0x7f, 0x34);
        assert_eq!(machine.cpu.regs.bx as u8, 0xf5);
    }

    #[test]
    fn mode_three_divisor_updates_at_the_next_output_transition() {
        let mut speaker = Speaker::new(0);
        speaker.write_port(0x43, 0x96);
        speaker.write_port(0x42, 10);
        speaker.write_port(0x61, 3);
        speaker.advance(3);
        speaker.write_port(0x42, 7);
        assert_eq!(speaker.sample(5), 0.12);
        assert_eq!(speaker.sample(6), -0.12);
        speaker.advance(7);
        speaker.write_port(0x42, 8);
        assert_eq!(speaker.sample(8), -0.12);
        assert_eq!(speaker.sample(9), 0.12);
        assert_eq!(speaker.sample(12), 0.12);
        assert_eq!(speaker.sample(13), -0.12);
    }

    #[test]
    fn rising_gate_loads_a_pending_mode_two_divisor_before_the_old_period_ends() {
        let mut speaker = Speaker::new(0);
        speaker.write_port(0x43, 0x94);
        speaker.write_port(0x42, 10);
        speaker.write_port(0x61, 3);
        speaker.advance(3);
        speaker.write_port(0x42, 4);
        assert!(speaker.output(9));
        assert!(!speaker.output(10));
        assert!(speaker.output(11));
        assert!(!speaker.output(14));

        speaker.write_port(0x61, 2);
        speaker.advance(4);
        speaker.write_port(0x61, 3);
        assert!(speaker.output(7));
        assert!(!speaker.output(8));
        assert!(speaker.output(9));
    }
}
