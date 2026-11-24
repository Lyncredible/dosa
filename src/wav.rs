use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

use dosa_dos::AUDIO_SAMPLE_RATE;

const MAX_DATA_BYTES: u32 = (u32::MAX - 36) & !1;

pub(crate) struct Recording {
    path: PathBuf,
    file: tempfile::NamedTempFile,
    bytes: u32,
}

impl Recording {
    pub fn start(path: PathBuf) -> io::Result<Self> {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        write_header(file.as_file_mut(), 0)?;
        Ok(Self {
            path,
            file,
            bytes: 0,
        })
    }

    /// Append samples and report whether the RIFF size limit was reached.
    pub fn append(&mut self, samples: &[f32]) -> io::Result<bool> {
        let available = ((MAX_DATA_BYTES - self.bytes) / 2) as usize;
        let mut pcm = Vec::with_capacity(samples.len().min(available) * 2);
        for &sample in samples.iter().take(available) {
            let sample = (sample.clamp(-1.0, 1.0) * 32768.0).round() as i16;
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        self.file.write_all(&pcm)?;
        self.bytes += pcm.len() as u32;
        Ok(self.bytes == MAX_DATA_BYTES)
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.file.rewind()?;
        write_header(self.file.as_file_mut(), self.bytes)?;
        self.file.persist(&self.path)?;
        Ok(())
    }
}

fn write_header(mut output: impl Write, data_bytes: u32) -> io::Result<()> {
    let mut header = [0; 44];
    header[..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    header[8..16].copy_from_slice(b"WAVEfmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    header[22..24].copy_from_slice(&1u16.to_le_bytes()); // Mono
    header[24..28].copy_from_slice(&AUDIO_SAMPLE_RATE.to_le_bytes());
    header[28..32].copy_from_slice(&(AUDIO_SAMPLE_RATE * 2).to_le_bytes());
    header[32..34].copy_from_slice(&2u16.to_le_bytes()); // Block alignment
    header[34..36].copy_from_slice(&16u16.to_le_bytes()); // Bits per sample
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    output.write_all(&header)
}
