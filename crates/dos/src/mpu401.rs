use std::{error::Error, io::Read, sync::Arc};

use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

#[path = "midi_snapshot.rs"]
mod snapshot;
use snapshot::{FingerprintReader, MidiChip};
pub(super) use snapshot::{MidiReplay, MidiSnapshot};

const DATA: u16 = 0x330;
const STATUS_COMMAND: u16 = 0x331;
const ACK: u8 = 0xfe;

#[derive(Default)]
pub(super) struct Mpu401 {
    synth: Option<MidiChip>,
    acknowledgements: u8,
    uart: bool,
    decoder: Decoder,
}

impl Mpu401 {
    pub(super) fn load(&mut self, source: &mut impl Read) -> Result<(), Box<dyn Error>> {
        let mut reader = FingerprintReader::new(source);
        let font = Arc::new(SoundFont::new(&mut reader)?);
        self.synth = Some(MidiChip::new(font, reader.finish())?);
        self.reset_uart();
        Ok(())
    }

    pub(super) fn reset(&mut self) {
        self.reset_uart();
        if let Some(chip) = &mut self.synth {
            chip.restart();
        }
    }

    fn reset_uart(&mut self) {
        self.acknowledgements = 0;
        self.uart = false;
        self.decoder = Decoder::default();
    }

    pub(super) fn read_port(&mut self, port: u16) -> Option<u8> {
        self.synth.as_ref()?;
        match port {
            DATA if self.acknowledgements != 0 => {
                self.acknowledgements -= 1;
                Some(ACK)
            }
            DATA => Some(0xff),
            STATUS_COMMAND => Some(if self.acknowledgements == 0 {
                0xbf
            } else {
                0x3f
            }),
            _ => None,
        }
    }

    pub(super) fn write_port(&mut self, port: u16, byte: u8) -> bool {
        let Some(synth) = &mut self.synth else {
            return false;
        };
        match port {
            STATUS_COMMAND if byte == 0xff => {
                synth.apply(Message::Reset);
                self.reset_uart();
                self.acknowledgements = 1;
            }
            STATUS_COMMAND if byte == 0x3f && !self.uart => {
                self.uart = true;
                self.acknowledgements += 1;
            }
            STATUS_COMMAND => {}
            DATA if self.uart => {
                if let Some(message) = self.decoder.push(byte) {
                    synth.apply(message);
                }
            }
            DATA => {}
            _ => return false,
        }
        true
    }

    pub(super) fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        if let Some(synth) = &mut self.synth {
            synth.render(left, right);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
enum Message {
    Channel(u8, [u8; 2]),
    Reset,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct Decoder {
    status: u8,
    data: [u8; 2],
    received: usize,
    sysex: Option<([u8; 8], usize)>,
}

impl Decoder {
    fn push(&mut self, byte: u8) -> Option<Message> {
        match byte {
            0xff => {
                *self = Self::default();
                Some(Message::Reset)
            }
            0xf8..=0xfe => None,
            0xf7 => {
                self.status = 0;
                self.received = 0;
                let (bytes, length) = self.sysex.take()?;
                let reset = matches!(
                    bytes.get(..length),
                    Some([0x7e, _, 0x09, 0x01 | 0x03] | [0x41, _, 0x42, 0x12, 0x40, 0, 0x7f, 0])
                );
                reset.then_some(Message::Reset)
            }
            0x80..=0xf6 => {
                self.status = if byte < 0xf0 { byte } else { 0 };
                self.data = [0; 2];
                self.received = 0;
                self.sysex = (byte == 0xf0).then_some(([0; 8], 0));
                None
            }
            _ if let Some((bytes, length)) = &mut self.sysex => {
                if let Some(value) = bytes.get_mut(*length) {
                    *value = byte;
                }
                *length = (*length + 1).min(bytes.len() + 1);
                None
            }
            _ if self.status != 0 => {
                self.data[self.received] = byte;
                self.received += 1;
                let size = if matches!(self.status & 0xf0, 0xc0 | 0xd0) {
                    1
                } else {
                    2
                };
                if self.received == size {
                    self.received = 0;
                    Some(Message::Channel(self.status, self.data))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RunState, tests::Directory};
    use std::time::Duration;

    #[test]
    fn midi_running_status_survives_realtime_and_ends_at_system_common() {
        let mut decoder = Decoder::default();
        let bytes = [0x92, 60, 0xf8, 63, 61, 64, 0xc2, 7, 12, 0xf7, 15];
        assert_eq!(
            bytes
                .into_iter()
                .filter_map(|byte| decoder.push(byte))
                .collect::<Vec<_>>(),
            [
                Message::Channel(0x92, [60, 63]),
                Message::Channel(0x92, [61, 64]),
                Message::Channel(0xc2, [7, 0]),
                Message::Channel(0xc2, [12, 0]),
            ]
        );
        assert_eq!(decoder.push(0xf1), None);
        assert_eq!(decoder.push(0), None);
        let gm_reset = [0xf0, 0x7e, 0x7f, 0xf8, 0x09, 1, 0xf7];
        assert_eq!(
            gm_reset
                .into_iter()
                .filter_map(|byte| decoder.push(byte))
                .collect::<Vec<_>>(),
            [Message::Reset]
        );
    }

    #[test]
    fn guest_mpu401_uart_generates_pcm_and_preserves_font_across_reset() {
        let folder = Directory::new("midi-uart");
        let guest = [
            0xba, 0x31, 0x03, 0xec, 0xa2, 0, 2, // Save idle status.
            0xb0, 0xff, 0xee, 0xec, 0xa2, 1, 2, // Reset and save status with ACK pending.
            0x4a, 0xec, 0xa2, 2, 2, 0x42, // Read reset ACK.
            0xb0, 0x3f, 0xee, 0x4a, 0xec, 0xa2, 3, 2, // Enable UART and read ACK.
            0xb0, 0x90, 0xee, 0xb0, 60, 0xee, 0xb0, 100, 0xee, // Note on.
            0x31, 0xc0, 0xcd, 0x16, 0xeb, 0xfe, // Wait for input.
        ];
        folder.write("MIDI.COM", &guest);
        let mut machine = folder.machine();
        assert_eq!(
            machine.audio.read_port(STATUS_COMMAND, Duration::ZERO),
            None
        );
        assert!(machine.load_midi_soundfont(&mut &b"invalid"[..]).is_err());
        machine
            .load_midi_soundfont(&mut test_soundfont().as_slice())
            .unwrap();

        for _ in 0..2 {
            machine.launch('C', "MIDI.COM", "").unwrap();
            assert_eq!(machine.run(100).unwrap(), RunState::WaitingForKey);
            let segment = machine.cpu.regs.ds;
            let status: [u8; 4] =
                std::array::from_fn(|index| machine.cpu.read_u8(segment, 0x200 + index as u16));
            assert_eq!(status, [0xbf, 0x3f, ACK, ACK]);
            let end = machine.elapsed() + Duration::from_millis(150);
            machine.audio.advance(end, &machine.cpu);
            let pcm = machine.take_audio();
            assert!(pcm.iter().any(|sample| sample.abs() > 0.02));
            machine.reset();
        }
    }

    #[test]
    fn midi_snapshot_restores_notes_uart_acknowledgements_and_running_status() {
        use crate::GuestSnapshotError;

        let folder = Directory::new("midi-save-state");
        folder.write("LOOP.COM", &[0xeb, 0xfe]);
        let font = test_soundfont();
        let mut original = folder.machine();
        original.load_midi_soundfont(&mut font.as_slice()).unwrap();
        original.launch('C', "LOOP.COM", "").unwrap();
        original.clock = std::time::Instant::now() - Duration::from_millis(100);
        for (port, value) in [
            (STATUS_COMMAND, 0xff),
            (STATUS_COMMAND, 0x3f),
            (DATA, 0x90),
            (DATA, 60),
            (DATA, 100),
        ] {
            assert!(original.audio.mpu.write_port(port, value));
        }
        original
            .audio
            .advance(Duration::from_millis(70), &original.cpu);
        assert!(original.take_audio().iter().any(|value| value.abs() > 0.02));
        assert!(original.audio.mpu.write_port(DATA, 67)); // A partial running-status Note On.
        original.set_paused(true);
        let bytes = original.save_guest().unwrap();

        let mut missing_font = folder.machine();
        assert!(matches!(
            missing_font.load_guest(&bytes),
            Err(GuestSnapshotError::MidiSoundFontChanged)
        ));
        let mut different_font = font.clone();
        let sample_data = different_font
            .windows(4)
            .position(|window| window == b"smpl")
            .unwrap();
        different_font[sample_data + 8] ^= 1;
        missing_font
            .load_midi_soundfont(&mut different_font.as_slice())
            .unwrap();
        assert!(matches!(
            missing_font.load_guest(&bytes),
            Err(GuestSnapshotError::MidiSoundFontChanged)
        ));

        let mut restored = folder.machine();
        restored.load_midi_soundfont(&mut font.as_slice()).unwrap();
        restored.load_guest(&bytes).unwrap();
        for guest in [&mut original, &mut restored] {
            let port = &mut guest.audio.mpu;
            assert_eq!(port.read_port(STATUS_COMMAND), Some(0x3f));
            assert_eq!(port.read_port(DATA), Some(ACK));
            assert_eq!(port.read_port(DATA), Some(ACK));
            assert_eq!(port.read_port(STATUS_COMMAND), Some(0xbf));
            assert!(port.write_port(DATA, 96));
            guest.audio.advance(Duration::from_millis(105), &guest.cpu);
        }
        let expected = original.take_audio();
        assert!(expected.iter().any(|value| value.abs() > 0.02));
        assert_eq!(restored.take_audio(), expected);
    }

    fn test_soundfont() -> Vec<u8> {
        fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
            let size = (data.len() as u32).to_le_bytes();
            [id, &size]
                .into_iter()
                .flatten()
                .copied()
                .chain(data.iter().copied())
                .collect()
        }

        fn list(kind: &[u8; 4], entries: &[Vec<u8>]) -> Vec<u8> {
            let mut data = kind.to_vec();
            for entry in entries {
                data.extend_from_slice(entry);
            }
            chunk(b"LIST", &data)
        }

        let mut preset = [0; 76]; // One preset plus the terminal record.
        preset[38 + 24] = 1;
        let mut instrument = [0; 44];
        instrument[22 + 20] = 1;
        let mut sample = [0; 92];
        sample[24..28].copy_from_slice(&480_u32.to_le_bytes());
        sample[28..32].copy_from_slice(&48_u32.to_le_bytes());
        sample[32..36].copy_from_slice(&432_u32.to_le_bytes());
        sample[36..40].copy_from_slice(&48_000_u32.to_le_bytes());
        sample[40] = 60;
        sample[44] = 1;
        let wave = (0..480)
            .map(|i| {
                let phase = std::f32::consts::TAU * i as f32 / 48.0;
                (phase.sin() * 20_000.0) as i16
            })
            .chain([0; 46])
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();

        let mut riff = b"sfbk".to_vec();
        riff.extend(list(b"INFO", &[chunk(b"ifil", &[2, 0, 1, 0])]));
        riff.extend(list(b"sdta", &[chunk(b"smpl", &wave)]));
        riff.extend(list(
            b"pdta",
            &[
                chunk(b"phdr", &preset),
                chunk(b"pbag", &[0, 0, 0, 0, 1, 0, 0, 0]),
                chunk(b"pmod", &[0; 10]),
                chunk(b"pgen", &[41, 0, 0, 0, 0, 0, 0, 0]), // Instrument 0.
                chunk(b"inst", &instrument),
                chunk(b"ibag", &[0, 0, 0, 0, 2, 0, 0, 0]),
                chunk(b"imod", &[0; 10]),
                chunk(b"igen", &[54, 0, 1, 0, 53, 0, 0, 0, 0, 0, 0, 0]), // Loop sample 0.
                chunk(b"shdr", &sample),
            ],
        ));
        chunk(b"RIFF", &riff)
    }
}
