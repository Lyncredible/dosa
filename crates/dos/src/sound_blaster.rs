//! Sound Blaster 2.0 DSP reset, commands, and unsigned 8-bit playback.

use std::collections::VecDeque;

use dosa_cpu::Cpu;

use crate::{audio::SAMPLE_RATE, dma::Dma};

pub(super) const IRQ: u8 = 7;
pub(super) const ENVIRONMENT: &str = "BLASTER=A220 I7 D1 T3";

pub(super) struct SoundBlaster {
    reset: bool,
    response: VecDeque<u8>,
    pending: Option<Command>,
    speaker: bool,
    dac: u8,
    period_us: u64,
    block_size: u64,
    transfer: Option<Transfer>,
    phase: u64,
    irq_line: bool,
    irq_pending: bool,
    test_register: u8,
    identification: u8,
    identification_mask: u8,
    identification_pending: bool,
}

struct Command {
    opcode: u8,
    args: [u8; 2],
    received: usize,
    length: usize,
}

struct Transfer {
    remaining: u64,
    auto_init: bool,
    paused: bool,
}

impl Default for SoundBlaster {
    fn default() -> Self {
        Self {
            reset: false,
            response: VecDeque::new(),
            pending: None,
            speaker: false,
            dac: 128,
            period_us: 256,
            block_size: 1,
            transfer: None,
            phase: 0,
            irq_line: false,
            irq_pending: false,
            test_register: 0,
            identification: 0xaa,
            identification_mask: 0x96,
            identification_pending: false,
        }
    }
}

impl SoundBlaster {
    pub fn read_port(&mut self, port: u16) -> Option<u8> {
        match port {
            0x22a => Some(self.response.pop_front().unwrap_or(0xff)),
            0x22c => Some(0x7f | (u8::from(self.reset) << 7)),
            0x22e => {
                self.irq_line = false;
                Some(0x7f | (u8::from(!self.response.is_empty()) << 7))
            }
            _ => None,
        }
    }

    pub fn write_port(&mut self, port: u16, value: u8) -> bool {
        match port {
            0x226 if value & 1 != 0 => {
                *self = Self {
                    reset: true,
                    irq_pending: self.irq_pending,
                    ..Self::default()
                };
            }
            0x226 if self.reset => {
                self.reset = false;
                self.response.push_back(0xaa);
            }
            0x22c if !self.reset => self.write_dsp(value),
            0x226 | 0x22c => (),
            _ => return false,
        }
        true
    }

    pub fn irq_requested(&self) -> bool {
        self.irq_pending
    }

    pub fn acknowledge_irq(&mut self) {
        self.irq_pending = false;
    }

    pub fn transfer_identification(&mut self, dma: &mut Dma, cpu: &mut Cpu) {
        if self.identification_pending && dma.write_byte(self.identification, cpu) {
            self.identification_pending = false;
        }
    }

    pub fn advance(&mut self, frames: u64, dma: &mut Dma, cpu: &Cpu) {
        let Some(transfer) = self.transfer.as_mut().filter(|transfer| !transfer.paused) else {
            return;
        };
        let period = u128::from(self.period_us * SAMPLE_RATE);
        let elapsed = u128::from(self.phase) + u128::from(frames) * 1_000_000;
        self.phase = (elapsed % period) as u64;
        let mut requested = u64::try_from(elapsed / period).unwrap_or(u64::MAX);
        if !transfer.auto_init {
            requested = requested.min(transfer.remaining);
        }
        let Some((transferred, sample)) = dma.read_memory(requested, cpu) else {
            return;
        };
        self.dac = sample;
        if transferred < transfer.remaining {
            transfer.remaining -= transferred;
        } else {
            if transfer.auto_init {
                transfer.remaining =
                    self.block_size - (transferred - transfer.remaining) % self.block_size;
            } else {
                self.transfer = None;
            }
            self.raise_irq();
        }
    }

    pub fn sample(&self) -> f32 {
        if self.speaker {
            (f32::from(self.dac) - 128.0) / 256.0
        } else {
            0.0
        }
    }

    fn write_dsp(&mut self, value: u8) {
        if let Some(mut command) = self.pending.take() {
            command.args[command.received] = value;
            command.received += 1;
            if command.received == command.length {
                self.execute(command.opcode, command.args);
            } else {
                self.pending = Some(command);
            }
            return;
        }
        let length = match value {
            0x10 | 0x40 | 0xe0 | 0xe2 | 0xe4 => 1,
            0x14 | 0x48 => 2,
            _ => 0,
        };
        if length == 0 {
            self.execute(value, [0; 2]);
        } else {
            self.pending = Some(Command {
                opcode: value,
                args: [0; 2],
                received: 0,
                length,
            });
        }
    }

    fn execute(&mut self, command: u8, args: [u8; 2]) {
        let length = u64::from(u16::from_le_bytes(args)) + 1;
        match command {
            0x10 => self.dac = args[0],
            0x14 => self.start(length, false),
            0x1c => self.start(self.block_size, true),
            0x40 => self.period_us = 256 - u64::from(args[0]),
            0x48 => self.block_size = length,
            0xd0 | 0xd4 => {
                if let Some(transfer) = &mut self.transfer {
                    transfer.paused = command == 0xd0;
                }
            }
            0xd1 => self.speaker = true,
            0xd3 => self.speaker = false,
            0xd8 => self.reply(&[if self.speaker { 0xff } else { 0 }]),
            0xda => {
                if let Some(transfer) = &mut self.transfer {
                    transfer.auto_init = false;
                }
            }
            0xe0 => self.reply(&[!args[0]]),
            0xe1 => self.reply(&[2, 0]),
            0xe2 => {
                self.identification = self
                    .identification
                    .wrapping_add(args[0] ^ self.identification_mask);
                self.identification_mask = self.identification_mask.rotate_right(2);
                self.identification_pending = true;
            }
            0xe4 => self.test_register = args[0],
            0xe8 => self.reply(&[self.test_register]),
            0xf2 => self.raise_irq(),
            _ => (),
        }
    }

    fn raise_irq(&mut self) {
        self.irq_pending |= !self.irq_line;
        self.irq_line = true;
    }

    fn reply(&mut self, data: &[u8]) {
        self.response.clear();
        self.response.extend(data);
    }

    fn start(&mut self, length: u64, auto_init: bool) {
        self.transfer = Some(Transfer {
            remaining: length,
            auto_init,
            paused: false,
        });
        self.phase = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsp_version_direct_dac_and_reset() {
        let mut dsp = SoundBlaster::default();
        dsp.write_port(0x22c, 0xe1);
        assert_eq!(dsp.read_port(0x22e), Some(0xff));
        assert_eq!(dsp.read_port(0x22a), Some(2));
        assert_eq!(dsp.read_port(0x22a), Some(0));
        assert_eq!(dsp.read_port(0x22e), Some(0x7f));
        dsp.write_port(0x22c, 0x10);
        dsp.write_port(0x22c, 0x00);
        assert_eq!(dsp.sample(), 0.0);
        dsp.write_port(0x22c, 0xd1);
        assert_eq!(dsp.sample(), -0.5);
        dsp.write_port(0x226, 1);
        assert_eq!(dsp.sample(), 0.0);
        dsp.write_port(0x226, 0);
        assert_eq!(dsp.read_port(0x22a), Some(0xaa));

        dsp.write_port(0x22c, 0xe4);
        dsp.write_port(0x22c, 0x5a);
        dsp.write_port(0x22c, 0xe8);
        assert_eq!(dsp.read_port(0x22e), Some(0xff));
        assert_eq!(dsp.read_port(0x22a), Some(0x5a));

        dsp.write_port(0x22c, 0xf2);
        assert!(dsp.irq_requested());
        dsp.read_port(0x22e);
        assert!(dsp.irq_requested());
        dsp.acknowledge_irq();
        assert!(!dsp.irq_requested());
    }

    #[test]
    fn identification_writes_to_guest_memory_when_dma_is_unmasked() {
        let mut dsp = SoundBlaster::default();
        let mut dma = Dma::default();
        let mut cpu = Cpu::default();
        for (port, byte) in [(2, 0x20), (2, 0), (3, 0), (3, 0), (0x83, 1), (0x0b, 0x55)] {
            dma.write_port(port, byte);
        }
        dsp.write_port(0x22c, 0xe2);
        dsp.write_port(0x22c, 0x06);
        dsp.transfer_identification(&mut dma, &mut cpu);
        assert_eq!(cpu.read_u8(0x1000, 0x20), 0);
        dma.write_port(0x0a, 1);
        dsp.transfer_identification(&mut dma, &mut cpu);
        assert_eq!(cpu.read_u8(0x1000, 0x20), 0x3a);
        dsp.write_port(0x22c, 0xe2);
        dsp.write_port(0x22c, 0x6b);
        dsp.transfer_identification(&mut dma, &mut cpu);
        assert_eq!(cpu.read_u8(0x1000, 0x20), 0x08);
        assert_eq!(dma.read_port(8), Some(2));
        assert!(!dsp.irq_requested());
    }

    #[test]
    fn auto_init_wraps_within_the_page_and_can_pause_or_finish() {
        let mut dsp = SoundBlaster::default();
        let mut dma = Dma::default();
        let mut cpu = Cpu::default();
        cpu.write_u8(0x2fff, 0xf, 64);
        cpu.write_u8(0x2000, 0, 192);
        for (port, byte) in [
            (2, 0xff),
            (2, 0xff),
            (3, 1),
            (3, 0),
            (0x83, 2),
            (0x0b, 0x59),
            (0xa, 1),
        ] {
            dma.write_port(port, byte);
        }
        for byte in [0xd1, 0x40, 156, 0x48, 1, 0, 0x1c] {
            dsp.write_port(0x22c, byte);
        }
        dsp.advance(4, &mut dma, &cpu);
        assert_eq!(dsp.sample(), 0.0);
        dsp.advance(1, &mut dma, &cpu);
        assert_eq!(dsp.sample(), -0.25);
        dsp.write_port(0x22c, 0xd0);
        dsp.advance(100, &mut dma, &cpu);
        assert!(!dsp.irq_requested());
        dsp.write_port(0x22c, 0xd4);
        dsp.advance(5, &mut dma, &cpu);
        assert_eq!(dsp.sample(), 0.25);
        assert!(dsp.irq_requested());
        dsp.acknowledge_irq();
        dsp.read_port(0x22e);
        dsp.advance(28, &mut dma, &cpu);
        assert_eq!(dsp.sample(), -0.25);
        assert!(dsp.irq_requested());
        dsp.acknowledge_irq();
        dsp.read_port(0x22e);
        dsp.write_port(0x22c, 0xda);
        dsp.advance(500, &mut dma, &cpu);
        assert_eq!(dsp.sample(), 0.25);
        assert!(dsp.irq_requested());
        dsp.acknowledge_irq();
        dsp.read_port(0x22e);
        dsp.advance(500, &mut dma, &cpu);
        assert!(!dsp.irq_requested());
    }
}
