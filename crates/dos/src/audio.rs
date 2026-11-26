//! Mix the guest audio devices into the desktop's mono PCM stream.

use std::{collections::VecDeque, time::Duration};

use dosa_cpu::Cpu;

use crate::{
    adlib::{Adlib, SynthReplay},
    dma::Dma,
    mpu401::Mpu401,
    pit,
    sound_blaster::SoundBlaster,
    speaker::Speaker,
};

pub(super) const SAMPLE_RATE: u64 = crate::AUDIO_SAMPLE_RATE as u64;
const MAX_SAMPLES: usize = SAMPLE_RATE as usize / 5;

pub(super) struct Audio {
    speaker: Speaker,
    adlib: Adlib,
    dma: Dma,
    sound_blaster: SoundBlaster,
    pub(super) mpu: Mpu401,
    pub(super) sample: u64,
    samples: VecDeque<f32>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct AudioSnapshot {
    speaker: Speaker,
    adlib: crate::adlib::AdlibSnapshot,
    dma: Dma,
    sound_blaster: SoundBlaster,
    sample: u64,
    samples: VecDeque<f32>,
}

impl Audio {
    pub(super) fn snapshot(&self) -> AudioSnapshot {
        AudioSnapshot {
            speaker: self.speaker.clone(),
            adlib: self.adlib.snapshot(),
            dma: self.dma.clone(),
            sound_blaster: self.sound_blaster.clone(),
            sample: self.sample,
            samples: self.samples.clone(),
        }
    }

    pub(super) fn adlib_replay(&self) -> Result<Option<&SynthReplay>, &'static str> {
        self.adlib.replay()
    }

    pub(super) fn from_snapshot(
        state: AudioSnapshot,
        elapsed: Duration,
        replay: Option<SynthReplay>,
    ) -> Option<Self> {
        if state.sample > sample_at(elapsed)
            || state.samples.len() > MAX_SAMPLES
            || !state
                .samples
                .iter()
                .all(|sample| (-1.0..=1.0).contains(sample))
            || !state.speaker.valid_snapshot(elapsed)
            || !state.sound_blaster.valid_snapshot()
        {
            return None;
        }
        Some(Self {
            speaker: state.speaker,
            adlib: Adlib::from_snapshot(state.adlib, replay, state.sample)?,
            dma: state.dma,
            sound_blaster: state.sound_blaster,
            mpu: Mpu401::default(),
            sample: state.sample,
            samples: state.samples,
        })
    }

    pub fn new(elapsed: Duration) -> Self {
        Self {
            speaker: Speaker::new(pit::ticks(elapsed)),
            adlib: Adlib::new(),
            dma: Dma::default(),
            sound_blaster: SoundBlaster::default(),
            mpu: Mpu401::default(),
            sample: sample_at(elapsed),
            samples: VecDeque::new(),
        }
    }

    pub fn restart(&mut self, elapsed: Duration) {
        let mut mpu = std::mem::take(&mut self.mpu);
        mpu.reset();
        *self = Self {
            mpu,
            ..Self::new(elapsed)
        };
    }

    pub fn advance(&mut self, elapsed: Duration, cpu: &Cpu) {
        let end = sample_at(elapsed);
        let start = self.sample.max(end.saturating_sub(MAX_SAMPLES as u64));
        if start > self.sample {
            self.sound_blaster
                .advance(start - self.sample, &mut self.dma, cpu);
            self.sample = start;
        }
        let mut left = [0.0; 64];
        let mut right = [0.0; 64];
        while self.sample < end {
            let count = (end - self.sample).min(left.len() as u64) as usize;
            self.mpu.render(&mut left[..count], &mut right[..count]);
            for (&left, &right) in left[..count].iter().zip(&right[..count]) {
                // Sample at the end of each PCM interval. The prior advance may
                // have committed a PIT reload partway through this interval.
                let tick =
                    (u128::from(self.sample + 1) * pit::FREQUENCY / u128::from(SAMPLE_RATE)) as u64;
                self.sound_blaster.advance(1, &mut self.dma, cpu);
                let value = (self.speaker.sample(tick)
                    + self.adlib.sample()
                    + self.sound_blaster.sample()
                    + (left + right) * 0.5)
                    .clamp(-1.0, 1.0);
                if self.samples.len() == MAX_SAMPLES {
                    self.samples.pop_front();
                }
                self.samples.push_back(value);
                self.sample += 1;
            }
        }
        self.speaker.advance(pit::ticks(elapsed));
    }

    pub fn drain(&mut self) -> Vec<f32> {
        self.samples.drain(..).collect()
    }

    pub fn irq_requested(&self) -> bool {
        self.sound_blaster.irq_requested()
    }

    pub fn acknowledge_irq(&mut self) {
        self.sound_blaster.acknowledge_irq();
    }

    pub fn read_port(&mut self, port: u16, elapsed: Duration) -> Option<u8> {
        self.speaker
            .read_port(port)
            .or_else(|| self.dma.read_port(port))
            .or_else(|| self.sound_blaster.read_port(port))
            .or_else(|| self.mpu.read_port(port))
            .or_else(|| {
                self.adlib
                    .read_port(adlib_port(port), elapsed.as_micros() as u64)
            })
    }

    pub fn write_port(&mut self, port: u16, value: u8, elapsed: Duration, cpu: &mut Cpu) -> bool {
        if port == 0x43 {
            // Let the PIT apply the same read-back command to channels 0 and 1.
            self.speaker.read_back(value);
        }
        if self.dma.write_port(port, value) || self.sound_blaster.write_port(port, value) {
            self.sound_blaster
                .transfer_identification(&mut self.dma, cpu);
            return true;
        }
        self.mpu.write_port(port, value)
            || self.speaker.write_port(port, value)
            || self
                .adlib
                .write_port(adlib_port(port), value, elapsed.as_micros() as u64)
    }
}

fn adlib_port(port: u16) -> u16 {
    match port {
        0x228 => 0x388,
        0x229 => 0x389,
        _ => port,
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
        machine.audio.advance(Duration::from_secs(1), &machine.cpu);
        let samples = machine.take_audio();
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert!(samples.iter().all(|sample| (-1.0..=1.0).contains(sample)));
        assert!(samples.iter().any(|sample| *sample > 0.125));
        assert!(samples.iter().any(|sample| *sample < 0.115));

        machine.stop();
        machine.launch('C', "IDLE.COM", "").unwrap();
        machine.audio.advance(Duration::from_secs(1), &machine.cpu);
        assert_eq!(machine.take_audio(), vec![0.0; MAX_SAMPLES]);
    }

    #[test]
    fn guest_dsp_reset_dma_playback_and_irq_acknowledgment() {
        guest_dma_playback(&[0x40, 156, 0x14, 63, 0]);
    }

    #[test]
    fn guest_high_speed_dma_playback_and_irq_acknowledgment() {
        guest_dma_playback(&[0x40, 233, 0x48, 63, 0, 0x91]);
    }

    fn guest_dma_playback(playback_commands: &[u8]) {
        let mut guest = vec![
            0xfa, // CLI
            0xb8, 0x0f, 0x25, 0xba, 0, 3, 0xcd, 0x21, // Set IRQ7 vector to DS:0300h
            0xba, 0x26, 0x02, 0xb0, 1, 0xee, 0x30, 0xc0, 0xee, // Reset DSP
            0xba, 0x2a, 0x02, 0xec, 0xa2, 1, 4, // Store reset response
            0xb0, 0x7f, 0xe6, 0x21, // Enable only IRQ7
        ];
        for (port, value) in [
            (0x0a, 5),
            (0x0c, 0),
            (0x02, 0),
            (0x02, 2),
            (0x83, 1),
            (0x03, 63),
            (0x03, 0),
            (0x0b, 0x49),
            (0x0a, 1),
        ] {
            guest.extend([0xb0, value, 0xe6, port]); // OUT immediate,AL
        }
        guest.extend([0xba, 0x2c, 0x02]);
        for value in std::iter::once(0xd1).chain(playback_commands.iter().copied()) {
            guest.extend([0xb0, value, 0xee]); // Enable DAC and play 64 bytes by DMA.
        }
        guest.extend([0xfb, 0xf4, 0xeb, 0xfd]); // STI; HLT; JMP back to HLT
        guest.resize(0x100, 0);
        guest.extend((0..64).map(|index| if index % 2 == 0 { 64 } else { 192 }));
        guest.resize(0x200, 0);
        guest.extend([
            0x50, 0x52, 0xba, 0x2e, 0x02, 0xec, // Read DSP status to acknowledge IRQ
            0xfe, 0x06, 0, 4, // INC BYTE [0400h]
            0xb0, 0x20, 0xe6, 0x20, // PIC EOI
            0x5a, 0x58, 0xcf, // Restore registers and IRET
        ]);
        let directory = Directory::new("sound-blaster");
        directory.write("PCM.COM", &guest);
        let mut machine = directory.machine();
        machine.launch('C', "PCM.COM", "").unwrap();
        let env = machine.cpu.read_u16(crate::loader::PSP, 0x2c);
        let env: Vec<_> = (0..64)
            .map(|offset| machine.cpu.read_u8(env, offset))
            .collect();
        assert!(
            env.split(|&byte| byte == 0)
                .any(|part| part == crate::sound_blaster::ENVIRONMENT.as_bytes())
        );

        machine.run(200).unwrap();
        assert_eq!(machine.cpu.read_u8(crate::loader::PSP, 0x401), 0xaa);
        let elapsed = machine.clock.elapsed() + Duration::from_millis(20);
        machine.audio.advance(elapsed, &machine.cpu);
        let samples = machine.take_audio();
        assert!(samples.contains(&-0.25) && samples.contains(&0.25));
        machine.run(100).unwrap();
        assert_eq!(machine.cpu.read_u8(crate::loader::PSP, 0x400), 1);
        assert!(!machine.audio.irq_requested());
        assert_eq!(machine.audio.read_port(0x08, elapsed), Some(2));
        assert_eq!(machine.audio.read_port(0x08, elapsed), Some(0));
        assert_eq!(machine.audio.read_port(0x22c, elapsed), Some(0x7f));
    }
}
