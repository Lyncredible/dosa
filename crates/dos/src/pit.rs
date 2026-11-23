use std::time::Duration;

pub(super) const BIOS_DIVISOR: u64 = 65_536;
const FREQUENCY: u128 = 1_193_182;

pub(super) fn ticks(elapsed: Duration) -> u64 {
    (elapsed.as_nanos() * FREQUENCY / 1_000_000_000) as u64
}

#[derive(Clone, Copy)]
enum Access {
    Low,
    High,
    Both,
}

pub(super) struct Pit {
    now: u64,
    next_edge: u64,
    period: Option<u64>,
    pending: bool,
    access: Access,
    periodic_mode: bool,
    low_byte: Option<u8>,
    read_high: bool,
    latched_count: Option<u16>,
}

impl Pit {
    pub fn new(now: u64) -> Self {
        Self {
            now,
            next_edge: now + BIOS_DIVISOR,
            period: Some(BIOS_DIVISOR),
            pending: false,
            access: Access::Both,
            periodic_mode: true,
            low_byte: None,
            read_high: false,
            latched_count: None,
        }
    }

    pub fn advance(&mut self, now: u64) {
        self.now = now;
        if let Some(period) = self.period
            && now >= self.next_edge
        {
            // The PIC retains a request when multiple edges arrive before service.
            self.pending = true;
            self.next_edge = now + period - (now - self.next_edge) % period;
        }
    }

    pub fn irq_requested(&self) -> bool {
        self.pending
    }

    pub fn acknowledge_irq(&mut self) {
        self.pending = false;
    }

    pub fn port_read(&mut self, port: u16) -> Option<u8> {
        if port != 0x40 {
            return None;
        }
        let [low, high] = self
            .latched_count
            .unwrap_or_else(|| self.count())
            .to_le_bytes();
        let byte = match self.access {
            Access::Low => low,
            Access::High => high,
            Access::Both if self.read_high => {
                self.read_high = false;
                high
            }
            Access::Both => {
                self.read_high = true;
                return Some(low);
            }
        };
        self.latched_count = None;
        Some(byte)
    }

    pub fn port_write(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x40 => {
                let divisor = match self.access {
                    Access::Low => u16::from(value),
                    Access::High => u16::from(value) << 8,
                    Access::Both => match self.low_byte.take() {
                        Some(low) => u16::from_le_bytes([low, value]),
                        None => {
                            self.low_byte = Some(value);
                            return true;
                        }
                    },
                };
                let period = if divisor == 0 {
                    BIOS_DIVISOR
                } else {
                    u64::from(divisor)
                };
                self.period = self.periodic_mode.then_some(period);
                self.next_edge = self.now + period;
            }
            0x43 if value & 0xc0 == 0 => self.control(value),
            0x43 => {} // Channels 1 and 2 and the 8254 read-back command.
            _ => return false,
        }
        true
    }

    fn control(&mut self, value: u8) {
        self.access = match (value >> 4) & 3 {
            0 => {
                if self.latched_count.is_none() {
                    self.latched_count = Some(self.count());
                    self.read_high = false;
                }
                return;
            }
            1 => Access::Low,
            2 => Access::High,
            _ => Access::Both,
        };
        self.periodic_mode = matches!((value >> 1) & 7, 2 | 3 | 6 | 7) && value & 1 == 0;
        self.low_byte = None;
        self.read_high = false;
        self.latched_count = None;
    }

    fn count(&self) -> u16 {
        if self.period.is_some() {
            (self.next_edge - self.now) as u16
        } else {
            0
        }
    }
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
        pit.advance(100 + BIOS_DIVISOR + 48);
        assert!(pit.irq_requested());
        pit.acknowledge_irq();
        pit.advance(100 + BIOS_DIVISOR + 63);
        assert!(!pit.irq_requested());
        pit.advance(100 + BIOS_DIVISOR + 64);
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
}
