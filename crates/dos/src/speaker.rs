//! PIT channel 2 and the PC speaker gate.

pub(super) struct Speaker {
    now: u64,
    epoch: u64,
    divisor: u32,
    access: u8,
    mode: u8,
    low: Option<u8>,
    read_high: bool,
    latched: Option<u16>,
    gate: u8,
}

impl Speaker {
    pub fn new(now: u64) -> Self {
        Self {
            now,
            epoch: now,
            divisor: 65536,
            access: 3,
            mode: 3,
            low: None,
            read_high: false,
            latched: None,
            gate: 0,
        }
    }

    pub fn advance(&mut self, now: u64) {
        self.now = now;
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
        let elapsed = now.saturating_sub(self.epoch);
        let divisor = u64::from(self.divisor);
        match self.mode {
            0 | 1 => elapsed >= divisor,
            2 => elapsed % divisor != divisor - 1,
            3 => elapsed % divisor < divisor.div_ceil(2),
            4 | 5 => elapsed != divisor,
            _ => true,
        }
    }

    fn count(&self) -> u16 {
        let elapsed = self.now.saturating_sub(self.epoch);
        let divisor = u64::from(self.divisor);
        let remaining = match self.mode {
            2 => divisor - elapsed % divisor,
            3 => {
                let phase = elapsed % divisor;
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
        remaining as u16
    }

    pub fn read_port(&mut self, port: u16) -> Option<u8> {
        match port {
            0x61 => Some((self.gate & !0x20) | (u8::from(self.output(self.now)) << 5)),
            0x42 => {
                let [low, high] = self.latched.unwrap_or_else(|| self.count()).to_le_bytes();
                let value = match self.access {
                    1 => low,
                    2 => high,
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
                    self.epoch = self.now;
                }
                self.gate = value;
            }
            0x43 if value & 0xc0 == 0x80 => {
                let access = (value >> 4) & 3;
                if access == 0 {
                    self.latched.get_or_insert(self.count());
                } else {
                    self.access = access;
                    let mode = value >> 1 & 7;
                    self.mode = if mode >= 6 { mode - 4 } else { mode };
                    self.low = None;
                    self.read_high = false;
                    self.latched = None;
                }
            }
            0x42 => {
                let divisor = match self.access {
                    1 => u16::from(value),
                    2 => u16::from(value) << 8,
                    _ => match self.low.take() {
                        Some(low) => u16::from_le_bytes([low, value]),
                        None => {
                            self.low = Some(value);
                            return true;
                        }
                    },
                };
                self.divisor = if divisor == 0 {
                    65536
                } else {
                    u32::from(divisor)
                };
                self.epoch = self.now;
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
        speaker.advance(1);
        assert_eq!(speaker.count(), 100);
        assert!(speaker.output(50));
        assert!(!speaker.output(51));
        speaker.advance(51);
        assert_eq!(speaker.count(), 100);
        assert!(speaker.output(101));
    }

    #[test]
    fn mode_two_has_a_single_clock_low_pulse_and_latched_reload() {
        let mut speaker = Speaker::new(0);
        speaker.write_port(0x43, 0xb4);
        speaker.write_port(0x42, 10);
        speaker.write_port(0x42, 0);
        speaker.write_port(0x61, 3);
        assert!(speaker.output(8));
        assert!(!speaker.output(9));
        assert!(speaker.output(10));
        speaker.advance(9);
        speaker.write_port(0x43, 0x80);
        assert_eq!(speaker.read_port(0x42), Some(1));
        speaker.advance(10);
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
        machine.audio.advance(std::time::Duration::from_secs(1));
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
        assert!(speaker.output(49));
        assert!(!speaker.output(50));
        assert!(speaker.output(100));
        speaker.advance(10);
        speaker.write_port(0x43, 0x80);
        assert_eq!(speaker.read_port(0x42), Some(80));
        speaker.advance(30);
        assert_eq!(speaker.read_port(0x42), Some(0));
        assert_eq!(speaker.sample(49), 0.12);
        assert_eq!(speaker.sample(50), -0.12);
        speaker.write_port(0x61, 0);
        assert_eq!(speaker.sample(100), 0.0);
    }
}
