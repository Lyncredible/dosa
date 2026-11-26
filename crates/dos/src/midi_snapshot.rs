use super::*;
use crate::GuestSnapshotError;
use serde::{Deserialize, Serialize};

const MAX_FRAMES: u32 = crate::AUDIO_SAMPLE_RATE * 30 * 60;
const MAX_MESSAGES: usize = 1_000_000;

pub(super) struct MidiChip {
    font: Arc<SoundFont>,
    identity: SoundFontIdentity,
    synth: Box<Synthesizer>,
    replay: Option<MidiReplay>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SoundFontIdentity {
    bytes: u64,
    checksum: u32,
}

pub(super) struct FingerprintReader<R> {
    reader: R,
    bytes: u64,
    hasher: crc32fast::Hasher,
}

impl<R: Read> FingerprintReader<R> {
    pub(super) fn new(reader: R) -> Self {
        Self {
            reader,
            bytes: 0,
            hasher: crc32fast::Hasher::new(),
        }
    }

    pub(super) fn finish(self) -> SoundFontIdentity {
        SoundFontIdentity {
            bytes: self.bytes,
            checksum: self.hasher.finalize(),
        }
    }
}

impl<R: Read> Read for FingerprintReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let len = self.reader.read(buffer)?;
        self.bytes += len as u64;
        self.hasher.update(&buffer[..len]);
        Ok(len)
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct MidiReplay {
    frames: u32,
    messages: Vec<(u32, Message)>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct MidiSnapshot<T> {
    identity: SoundFontIdentity,
    acknowledgements: u8,
    uart: bool,
    decoder: Decoder,
    replay: T,
}

impl MidiChip {
    pub(super) fn new(
        font: Arc<SoundFont>,
        identity: SoundFontIdentity,
    ) -> Result<Self, rustysynth::SynthesizerError> {
        let synth = Synthesizer::new(
            &font,
            &SynthesizerSettings::new(crate::AUDIO_SAMPLE_RATE as i32),
        )?;
        Ok(Self {
            font,
            identity,
            synth: Box::new(synth),
            replay: Some(MidiReplay::default()),
        })
    }

    pub(super) fn restart(&mut self) {
        *self = Self::new(Arc::clone(&self.font), self.identity)
            .expect("the SoundFont and settings already initialized successfully");
    }

    pub(super) fn apply(&mut self, message: Message) {
        if let Some(replay) = &mut self.replay {
            if replay.messages.len() == MAX_MESSAGES {
                self.replay = None;
            } else {
                replay.messages.push((replay.frames, message));
            }
        }
        apply_message(&mut self.synth, message);
    }

    pub(super) fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        if let Some(replay) = &mut self.replay {
            let frames = replay.frames.saturating_add(left.len() as u32);
            if frames > MAX_FRAMES {
                self.replay = None;
            } else {
                replay.frames = frames;
            }
        }
        self.synth.render(left, right);
    }
}

fn apply_message(synth: &mut Synthesizer, message: Message) {
    match message {
        Message::Channel(status, [first, second]) => synth.process_midi_message(
            i32::from(status & 15),
            i32::from(status & 0xf0),
            i32::from(first),
            i32::from(second),
        ),
        Message::Reset => synth.reset(),
    }
}

fn advance_silently(synth: &mut Synthesizer, frames: u32) {
    let mut left = [0.0; 64];
    let mut right = [0.0; 64];
    let mut frames = frames as usize;
    while frames != 0 {
        let count = frames.min(left.len());
        synth.render(&mut left[..count], &mut right[..count]);
        frames -= count;
    }
}

impl Mpu401 {
    pub(crate) fn snapshot(&self) -> Result<Option<MidiSnapshot<&MidiReplay>>, &'static str> {
        let Some(chip) = &self.synth else {
            return Ok(None);
        };
        Ok(Some(MidiSnapshot {
            identity: chip.identity,
            acknowledgements: self.acknowledgements,
            uart: self.uart,
            decoder: self.decoder.clone(),
            replay: chip
                .replay
                .as_ref()
                .ok_or("MIDI replay exceeds the save limit")?,
        }))
    }

    pub(crate) fn from_snapshot(
        saved: Option<MidiSnapshot<MidiReplay>>,
        configured: &Self,
        rendered_frames: u64,
    ) -> Result<Self, GuestSnapshotError> {
        use GuestSnapshotError::{Invalid, MidiSoundFontChanged};
        let (saved, current) = match (saved, &configured.synth) {
            (Some(saved), Some(current)) if saved.identity == current.identity => (saved, current),
            (None, None) => return Ok(Self::default()),
            _ => return Err(MidiSoundFontChanged),
        };
        let replay = saved.replay;
        if saved.acknowledgements > 2
            || !saved.decoder.valid_snapshot()
            || replay.frames > MAX_FRAMES
            || u64::from(replay.frames) > rendered_frames
            || replay.messages.len() > MAX_MESSAGES
        {
            return Err(Invalid);
        }
        let mut prior = 0;
        for &(frame, message) in &replay.messages {
            if !(prior..=replay.frames).contains(&frame)
                || matches!(message, Message::Channel(status, data)
                    if !(0x80..=0xef).contains(&status) || !data.iter().all(|&byte| byte < 128))
            {
                return Err(Invalid);
            }
            prior = frame;
        }
        let mut chip =
            MidiChip::new(Arc::clone(&current.font), current.identity).map_err(|_| Invalid)?;
        let mut at = 0;
        for &(frame, message) in &replay.messages {
            advance_silently(&mut chip.synth, frame - at);
            apply_message(&mut chip.synth, message);
            at = frame;
        }
        advance_silently(&mut chip.synth, replay.frames - at);
        chip.replay = Some(replay);
        Ok(Self {
            synth: Some(chip),
            acknowledgements: saved.acknowledgements,
            uart: saved.uart,
            decoder: saved.decoder,
        })
    }
}

impl Decoder {
    fn valid_snapshot(&self) -> bool {
        let message_bytes = match self.status {
            0 => 1,
            0xc0..=0xdf => 1,
            0x80..=0xef => 2,
            _ => return false,
        };
        self.received < message_bytes
            && self.data.iter().all(|&byte| byte < 128)
            && self.sysex.as_ref().is_none_or(|(data, length)| {
                self.status == 0 && *length <= data.len() + 1 && data.iter().all(|&byte| byte < 128)
            })
    }
}
