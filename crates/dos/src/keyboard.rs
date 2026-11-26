use std::collections::VecDeque;

pub(super) const KEYBOARD_VECTOR: u8 = 0x09;
pub(super) const MAX_KEYS: usize = 32;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Scancode {
    value: u8,
    requested: bool,
    bios_key: Option<(u8, u8)>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Keyboard {
    output: VecDeque<Scancode>,
    guest_handler: bool,
    system_control: u8,
    interface_disabled: bool,
    output_port: u8,
    output_port_pending: bool,
    controller_reply: Option<u8>,
}

impl Default for Keyboard {
    fn default() -> Self {
        Self {
            output: VecDeque::new(),
            guest_handler: false,
            system_control: 0,
            interface_disabled: false,
            output_port: 1,
            output_port_pending: false,
            controller_reply: None,
        }
    }
}

impl Keyboard {
    pub(super) fn valid_snapshot(&self) -> bool {
        self.output.len() <= MAX_KEYS
    }

    pub fn a20_enabled(&self) -> bool {
        self.output_port & 2 != 0
    }

    pub fn set_a20(&mut self, enabled: bool) {
        self.output_port = self.output_port & !2 | (u8::from(enabled) << 1);
    }

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
            0x60 => self
                .controller_reply
                .take()
                .unwrap_or_else(|| self.output.pop_front().map_or(0, |code| code.value)),
            0x61 => self.system_control,
            0x64 => u8::from(self.controller_reply.is_some() || !self.output.is_empty()),
            _ => return None,
        })
    }

    pub fn port_write(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x61 => self.system_control = value,
            0x60 if self.output_port_pending => {
                self.output_port = value;
                self.output_port_pending = false;
            }
            0x64 => {
                self.output_port_pending = value == 0xd1;
                match value {
                    0xad => self.interface_disabled = true,
                    0xae => self.interface_disabled = false,
                    0xd0 => self.controller_reply = Some(self.output_port),
                    _ => {}
                }
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
