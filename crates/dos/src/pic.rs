#[derive(Default)]
pub(super) struct Pic {
    mask: u8,
    in_service: u8,
    read_isr: bool,
}

impl Pic {
    pub fn next_irq(&self, requests: u8) -> Option<u8> {
        let irq = (requests & !self.mask).trailing_zeros();
        // Fixed priority: a numerically lower IRQ can preempt the current one.
        (irq < self.in_service.trailing_zeros()).then_some(irq as u8)
    }

    pub fn acknowledge(&mut self, irq: u8) {
        self.in_service |= 1 << irq;
    }

    pub fn port_read(&self, port: u16, requests: u8) -> Option<u8> {
        match port {
            0x20 => Some(if self.read_isr {
                self.in_service
            } else {
                requests
            }),
            0x21 => Some(self.mask),
            _ => None,
        }
    }

    pub fn port_write(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x20 => match value {
                0x0a => self.read_isr = false,
                0x0b => self.read_isr = true,
                0x20 | 0xa0 => self.in_service &= self.in_service.wrapping_sub(1),
                0x60..=0x67 | 0xe0..=0xe7 => self.in_service &= !(1 << (value & 7)),
                _ => {}
            },
            0x21 => self.mask = value,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_priority_irq_can_preempt_and_eoi_ends_only_one_irq() {
        let mut pic = Pic::default();
        assert_eq!(pic.next_irq(0b11), Some(0));
        pic.port_write(0x21, 1);
        assert_eq!(pic.next_irq(0b11), Some(1));
        pic.acknowledge(1);

        pic.port_write(0x21, 0);
        assert_eq!(pic.next_irq(0b11), Some(0));
        pic.acknowledge(0);
        assert_eq!(pic.next_irq(0b11), None);

        pic.port_write(0x20, 0x0b);
        assert_eq!(pic.port_read(0x20, 0), Some(0b11));
        pic.port_write(0x20, 0x20);
        assert_eq!(pic.port_read(0x20, 0), Some(0b10));
        assert_eq!(pic.next_irq(0b10), None);
        pic.port_write(0x20, 0x60);
        assert_eq!(pic.port_read(0x20, 0), Some(0b10));
        pic.port_write(0x20, 0x61);
        assert_eq!(pic.next_irq(0b10), Some(1));
    }
}
