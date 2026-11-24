//! Mix PC speaker and FM audio into the desktop's mono PCM stream.

use std::{collections::VecDeque, time::Duration};

use crate::{adlib::Adlib, pit, speaker::Speaker};

pub(super) const SAMPLE_RATE: u64 = 48_000;
const MAX_SAMPLES: usize = SAMPLE_RATE as usize / 5;

pub(super) struct Audio {
    speaker: Speaker,
    adlib: Adlib,
    sample: u64,
    samples: VecDeque<f32>,
}

impl Audio {
    pub fn new(elapsed: Duration) -> Self {
        Self {
            speaker: Speaker::new(pit::ticks(elapsed)),
            adlib: Adlib::new(),
            sample: sample_at(elapsed),
            samples: VecDeque::new(),
        }
    }

    pub fn advance(&mut self, elapsed: Duration) {
        let end = sample_at(elapsed);
        self.sample = self.sample.max(end.saturating_sub(MAX_SAMPLES as u64));
        while self.sample < end {
            let tick = (u128::from(self.sample) * pit::FREQUENCY / u128::from(SAMPLE_RATE)) as u64;
            let value = (self.speaker.sample(tick) + self.adlib.sample()).clamp(-1.0, 1.0);
            if self.samples.len() == MAX_SAMPLES {
                self.samples.pop_front();
            }
            self.samples.push_back(value);
            self.sample += 1;
        }
        self.speaker.advance(pit::ticks(elapsed));
    }

    pub fn drain(&mut self) -> Vec<f32> {
        self.samples.drain(..).collect()
    }

    pub fn read_port(&mut self, port: u16, elapsed: Duration) -> Option<u8> {
        self.speaker
            .read_port(port)
            .or_else(|| self.adlib.read_port(port, elapsed.as_micros() as u64))
    }

    pub fn write_port(&mut self, port: u16, value: u8, elapsed: Duration) -> bool {
        self.speaker.write_port(port, value)
            || self
                .adlib
                .write_port(port, value, elapsed.as_micros() as u64)
    }
}

fn sample_at(elapsed: Duration) -> u64 {
    (elapsed.as_nanos() * u128::from(SAMPLE_RATE) / 1_000_000_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Directory;

    #[test]
    fn guest_fm_register_writes_mix_with_pc_speaker_and_reset_on_launch() {
        let mut guest = vec![0xba, 0x88, 0x03]; // MOV DX,0388h
        for (register, value) in [
            (0x20, 0x01),
            (0x23, 0x01),
            (0x40, 0x18),
            (0x43, 0x00),
            (0x60, 0xf0),
            (0x63, 0xf0),
            (0x80, 0x77),
            (0x83, 0x77),
            (0xa0, 0x98),
            (0xb0, 0x31),
        ] {
            guest.extend([0xb8, register, value, 0xef]); // MOV AX, data:address; OUT DX,AX
        }
        guest.extend([
            0xec, 0x88, 0xc3, 0xe4, 0x61, 0x0c, 0x02, 0xe6, 0x61, 0xeb, 0xfe,
        ]);
        let directory = Directory::new("adlib");
        directory.write("FM.COM", &guest);
        directory.write("IDLE.COM", &[0xeb, 0xfe]);
        let mut machine = directory.machine();
        machine.launch('C', "FM.COM", "").unwrap();
        machine.run(40).unwrap();
        assert_eq!(machine.cpu.regs.bx as u8, 0x06);
        machine.audio.advance(Duration::from_secs(1));
        let samples = machine.take_audio();
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert!(samples.iter().all(|sample| (-1.0..=1.0).contains(sample)));
        assert!(samples.iter().any(|sample| *sample > 0.125));
        assert!(samples.iter().any(|sample| *sample < 0.115));

        machine.stop();
        machine.launch('C', "IDLE.COM", "").unwrap();
        machine.audio.advance(Duration::from_secs(1));
        assert_eq!(machine.take_audio(), vec![0.0; MAX_SAMPLES]);
    }
}
