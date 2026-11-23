use std::collections::VecDeque;

pub(super) const KEYBOARD_VECTOR: u8 = 0x09;
pub(super) const MAX_KEYS: usize = 32;

struct Scancode {
    value: u8,
    requested: bool,
    bios_key: Option<(u8, u8)>,
}

#[derive(Default)]
pub(super) struct Keyboard {
    output: VecDeque<Scancode>,
    guest_handler: bool,
    system_control: u8,
    interface_disabled: bool,
}

impl Keyboard {
    pub fn push(&mut self, value: u8, guest_handler: bool, bios_key: Option<(u8, u8)>) {
        if self.guest_handler != guest_handler {
            // BIOS input is delivered separately. Do not replay copies retained for
            // raw polling when a program takes over the IRQ1 vector.
            self.output.clear();
            self.guest_handler = guest_handler;
        }
        if value != 0 && self.output.len() < MAX_KEYS {
            self.output.push_back(Scancode {
                value,
                requested: true,
                bios_key,
            });
        }
    }

    pub fn read_bios_key(&mut self) -> Option<(u8, u8)> {
        self.output.pop_front().and_then(|code| code.bios_key)
    }

    pub fn irq_requested(&self) -> bool {
        !self.interface_disabled && self.output.front().is_some_and(|code| code.requested)
    }

    pub fn acknowledge_irq(&mut self) {
        if let Some(code) = self.output.front_mut() {
            code.requested = false;
        }
    }

    pub fn port_read(&mut self, port: u16) -> Option<u8> {
        Some(match port {
            0x60 => self.output.pop_front().map_or(0, |code| code.value),
            0x61 => self.system_control,
            0x64 => u8::from(!self.output.is_empty()),
            _ => return None,
        })
    }

    pub fn port_write(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x61 => self.system_control = value,
            0x64 => match value {
                0xad => self.interface_disabled = true,
                0xae => self.interface_disabled = false,
                _ => {}
            },
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_scancode_requests_only_one_irq() {
        let mut keyboard = Keyboard::default();
        keyboard.push(0x1e, true, Some((b'a', 0x1e)));
        keyboard.push(0x9e, true, None);
        assert!(keyboard.irq_requested());
        keyboard.acknowledge_irq();
        assert!(
            !keyboard.irq_requested(),
            "unread data does not request another IRQ"
        );
        assert_eq!(keyboard.port_read(0x60), Some(0x1e));
        assert!(keyboard.irq_requested());
        keyboard.acknowledge_irq();
        assert_eq!(keyboard.port_read(0x60), Some(0x9e));
        assert!(!keyboard.irq_requested());
    }

    #[test]
    fn raw_polling_works_without_replaying_bios_input_to_a_later_handler() {
        let mut keyboard = Keyboard::default();
        keyboard.push(0x30, false, None);
        assert!(keyboard.irq_requested());
        assert_eq!(keyboard.port_read(0x64), Some(1));
        assert_eq!(keyboard.port_read(0x60), Some(0x30));
        keyboard.push(0xb0, false, None);
        keyboard.push(0x1e, true, Some((b'a', 0x1e)));
        assert!(keyboard.irq_requested());
        assert_eq!(keyboard.port_read(0x60), Some(0x1e));
        assert_eq!(keyboard.port_read(0x64), Some(0));
    }
}
