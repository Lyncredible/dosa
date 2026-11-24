//! The XT DMA controller's channel 1, used for Sound Blaster playback.

use dosa_cpu::Cpu;

pub(super) struct Dma {
    address: u16,
    count: u16,
    base_address: u16,
    base_count: u16,
    page: u8,
    mode: u8,
    high_byte: bool,
    masked: bool,
    disabled: bool,
    terminal_count: bool,
}

impl Default for Dma {
    fn default() -> Self {
        Self {
            address: 0,
            count: 0,
            base_address: 0,
            base_count: 0,
            page: 0,
            mode: 0,
            high_byte: false,
            masked: true,
            disabled: false,
            terminal_count: false,
        }
    }
}

impl Dma {
    pub fn read_port(&mut self, port: u16) -> Option<u8> {
        match port {
            0x02 | 0x03 => {
                let register = if port == 0x02 {
                    self.address
                } else {
                    self.count
                };
                let index = usize::from(self.high_byte);
                self.high_byte = !self.high_byte;
                Some(register.to_le_bytes()[index])
            }
            0x08 => Some(u8::from(std::mem::take(&mut self.terminal_count)) << 1),
            0x83 => Some(self.page),
            _ => None,
        }
    }

    pub fn write_port(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x02 | 0x03 => {
                let (current, base) = if port == 0x02 {
                    (&mut self.address, &mut self.base_address)
                } else {
                    (&mut self.count, &mut self.base_count)
                };
                *current = replace_byte(*current, self.high_byte, value);
                *base = replace_byte(*base, self.high_byte, value);
                self.high_byte = !self.high_byte;
            }
            0x08 => self.disabled = value & 4 != 0,
            0x0a if value & 3 == 1 => self.masked = value & 4 != 0,
            0x0b if value & 3 == 1 => self.mode = value,
            0x0a | 0x0b => (),
            0x0c => self.high_byte = false,
            0x0d => {
                self.disabled = false;
                self.masked = true;
                self.high_byte = false;
                self.terminal_count = false;
            }
            0x0e => self.masked = false,
            0x0f => self.masked = value & 2 != 0,
            0x83 => self.page = value,
            _ => return false,
        }
        true
    }

    /// Consume playback requests and return the count and last DAC byte.
    pub fn read_memory(&mut self, requested: u64, cpu: &Cpu) -> Option<(u64, u8)> {
        let (count, address) = self.transfer(requested, 0x08)?;
        let (segment, offset) = self.memory_address(address);
        Some((count, cpu.read_u8(segment, offset)))
    }

    pub fn write_byte(&mut self, byte: u8, cpu: &mut Cpu) -> bool {
        let Some((_, address)) = self.transfer(1, 0x04) else {
            return false;
        };
        let (segment, offset) = self.memory_address(address);
        cpu.write_u8(segment, offset, byte);
        true
    }

    fn transfer(&mut self, requested: u64, direction: u8) -> Option<(u64, u16)> {
        if requested == 0
            || self.masked
            || self.disabled
            || self.mode & 0xc0 == 0xc0
            || self.mode & 0x0c != direction
        {
            return None;
        }
        let remaining = u64::from(self.count) + 1;
        let transferred = requested.min(remaining);
        let mut last_address = self.step_address(self.address, transferred - 1);
        self.address = self.step_address(self.address, transferred);
        self.count = self.count.wrapping_sub(transferred as u16);
        let mut total = transferred;
        if transferred == remaining {
            self.terminal_count = true;
            if self.mode & 0x10 == 0 {
                self.masked = true;
            } else {
                let block = u64::from(self.base_count) + 1;
                let extra = requested - transferred;
                let tail = extra % block;
                if extra != 0 {
                    last_address = self.step_address(self.base_address, (extra - 1) % block);
                }
                self.address = self.step_address(self.base_address, tail);
                self.count = self.base_count - tail as u16;
                total = requested;
            }
        }
        Some((total, last_address))
    }

    fn memory_address(&self, address: u16) -> (u16, u16) {
        let segment = (u16::from(self.page & 0xf) << 12) | (address >> 4);
        (segment, address & 0xf)
    }

    fn step_address(&self, address: u16, count: u64) -> u16 {
        if self.mode & 0x20 == 0 {
            address.wrapping_add(count as u16)
        } else {
            address.wrapping_sub(count as u16)
        }
    }
}

fn replace_byte(word: u16, high: bool, byte: u8) -> u16 {
    let mut bytes = word.to_le_bytes();
    bytes[usize::from(high)] = byte;
    u16::from_le_bytes(bytes)
}
