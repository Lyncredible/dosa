//! AdLib's YM3812 address/data ports and two polled hardware timers.

use oplon::Opl2;

use crate::audio::SAMPLE_RATE;

const MAX_REPLAY_FRAMES: u32 = SAMPLE_RATE as u32 * 30 * 60;
const MAX_REPLAY_WRITES: usize = 1_000_000;

pub(super) struct Adlib {
    address: u8,
    timers: [Timer; 2],
    synth: Option<Synth>,
}

struct Synth {
    chip: Opl2,
    replay: Option<SynthReplay>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct SynthReplay {
    frames: u32,
    writes: Vec<RegisterWrite>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RegisterWrite {
    frame: u32,
    address: u8,
    value: u8,
}

impl Synth {
    fn new() -> Self {
        Self {
            chip: Opl2::new(SAMPLE_RATE as u32),
            replay: Some(SynthReplay::default()),
        }
    }

    fn write(&mut self, address: u8, value: u8) {
        if let Some(replay) = &mut self.replay {
            if replay.writes.len() == MAX_REPLAY_WRITES {
                self.replay = None;
            } else {
                replay.writes.push(RegisterWrite {
                    frame: replay.frames,
                    address,
                    value,
                });
            }
        }
        self.chip.write_reg(address, value);
    }

    fn sample(&mut self) -> f32 {
        if let Some(replay) = &mut self.replay {
            if replay.frames == MAX_REPLAY_FRAMES {
                self.replay = None;
            } else {
                replay.frames += 1;
            }
        }
        let (mono, _) = self.chip.render_frame();
        mono as f32 / 32_768.0
    }
}

impl SynthReplay {
    fn restore(self, audio_frame: u64) -> Option<Synth> {
        if self.frames > MAX_REPLAY_FRAMES
            || u64::from(self.frames) > audio_frame
            || self.writes.len() > MAX_REPLAY_WRITES
            || self.writes.first()?.frame != 0
        {
            return None;
        }
        let mut current = 0;
        for write in &self.writes {
            if !(current..=self.frames).contains(&write.frame) || (2..=4).contains(&write.address) {
                return None;
            }
            current = write.frame;
        }

        // The chip exposes register writes and rendering. Replay them at the
        // same output frame boundaries to recover envelopes and sample phase.
        let mut chip = Opl2::new(SAMPLE_RATE as u32);
        let mut current = 0;
        for write in &self.writes {
            for _ in current..write.frame {
                chip.render_frame();
            }
            chip.write_reg(write.address, write.value);
            current = write.frame;
        }
        for _ in current..self.frames {
            chip.render_frame();
        }
        Some(Synth {
            chip,
            replay: Some(self),
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct AdlibSnapshot {
    address: u8,
    timers: [Timer; 2],
}

impl Adlib {
    pub(super) fn snapshot(&self) -> AdlibSnapshot {
        AdlibSnapshot {
            address: self.address,
            timers: self.timers,
        }
    }

    pub(super) fn replay(&self) -> Result<Option<&SynthReplay>, &'static str> {
        match &self.synth {
            None => Ok(None),
            Some(synth) => synth
                .replay
                .as_ref()
                .map(Some)
                .ok_or("AdLib replay exceeds the save limit"),
        }
    }

    pub(super) fn from_snapshot(
        state: AdlibSnapshot,
        replay: Option<SynthReplay>,
        audio_frame: u64,
    ) -> Option<Self> {
        if state.timers[0].quantum != 80 || state.timers[1].quantum != 320 {
            return None;
        }
        let synth = match replay {
            Some(replay) => Some(replay.restore(audio_frame)?),
            None => None,
        };
        Some(Self {
            address: state.address,
            timers: state.timers,
            synth,
        })
    }

    pub fn new() -> Self {
        Self {
            address: 0,
            timers: [Timer::new(80), Timer::new(320)],
            synth: None,
        }
    }

    pub fn read_port(&mut self, port: u16, now: u64) -> Option<u8> {
        match port {
            0x388 => {
                for timer in &mut self.timers {
                    timer.advance(now);
                }
                let flags = (u8::from(self.timers[0].overflow) << 6)
                    | (u8::from(self.timers[1].overflow) << 5);
                Some(0x06 | flags | (u8::from(flags != 0) << 7))
            }
            0x389 => Some(0xff),
            _ => None,
        }
    }

    pub fn write_port(&mut self, port: u16, value: u8, now: u64) -> bool {
        match port {
            0x388 => self.address = value,
            0x389 => match self.address {
                0x02 | 0x03 => {
                    let timer = &mut self.timers[usize::from(self.address - 2)];
                    timer.advance(now);
                    timer.counter = value;
                }
                0x04 => {
                    for (index, timer) in self.timers.iter_mut().enumerate() {
                        timer.advance(now);
                        if value & 0x80 != 0 {
                            timer.overflow = false;
                        } else {
                            timer.set_control(
                                now,
                                value & (1 << index) != 0,
                                value & (0x40 >> index) != 0,
                            );
                        }
                    }
                }
                address => self
                    .synth
                    .get_or_insert_with(Synth::new)
                    .write(address, value),
            },
            _ => return false,
        }
        true
    }

    pub fn sample(&mut self) -> f32 {
        self.synth.as_mut().map_or(0.0, Synth::sample)
    }
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
struct Timer {
    quantum: u64,
    counter: u8,
    deadline: Option<u64>,
    masked: bool,
    overflow: bool,
}

impl Timer {
    fn new(quantum: u64) -> Self {
        Self {
            quantum,
            counter: 0,
            deadline: None,
            masked: false,
            overflow: false,
        }
    }

    fn period(&self) -> u64 {
        (256 - u64::from(self.counter)) * self.quantum
    }

    fn advance(&mut self, now: u64) {
        if let Some(deadline) = self.deadline.filter(|&deadline| deadline <= now) {
            self.overflow |= !self.masked;
            let period = self.period();
            self.deadline = Some(now.saturating_add(period - (now - deadline) % period));
        }
    }

    fn set_control(&mut self, now: u64, started: bool, masked: bool) {
        self.masked = masked;
        if masked {
            self.overflow = false;
        }
        if started {
            let deadline = now.saturating_add(self.period());
            self.deadline.get_or_insert(deadline);
        } else {
            self.deadline = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timers_latch_mask_reset_and_reload_independently() {
        let mut adlib = Adlib::new();
        let write = |adlib: &mut Adlib, register, value, now| {
            assert!(adlib.write_port(0x388, register, now));
            assert!(adlib.write_port(0x389, value, now));
        };
        write(&mut adlib, 2, 0xff, 0);
        write(&mut adlib, 3, 0xff, 0);
        write(&mut adlib, 4, 0x03, 0);
        assert_eq!(adlib.read_port(0x388, 79), Some(0x06));
        assert_eq!(adlib.read_port(0x388, 80), Some(0xc6));
        write(&mut adlib, 4, 0x83, 100);
        assert_eq!(adlib.read_port(0x388, 159), Some(0x06));
        assert_eq!(adlib.read_port(0x388, 160), Some(0xc6));
        write(&mut adlib, 4, 0x43, 170);
        assert_eq!(adlib.read_port(0x388, 319), Some(0x06));
        assert_eq!(adlib.read_port(0x388, 320), Some(0xa6));
        write(&mut adlib, 4, 0x60, 321);
        assert_eq!(adlib.read_port(0x388, 1000), Some(0x06));
        assert_eq!(adlib.read_port(0x389, 1000), Some(0xff));
    }
}
