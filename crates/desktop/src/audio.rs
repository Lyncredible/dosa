use std::collections::VecDeque;
use std::error::Error;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

const SOURCE_RATE: f64 = 48_000.0;
const MAX_QUEUED: usize = 9_600;

pub(super) struct Audio {
    queue: Arc<Mutex<Queue>>,
    _stream: cpal::Stream,
}

struct Queue {
    samples: VecDeque<f32>,
    phase: f64,
    #[cfg(test)]
    played_samples: usize,
}

impl Audio {
    pub fn open() -> Result<Self, Box<dyn Error>> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no default audio output device")?;
        let supported = device.default_output_config()?;
        let format = supported.sample_format();
        let config = supported.config();
        let queue = Arc::new(Mutex::new(Queue {
            samples: VecDeque::new(),
            phase: 0.0,
            #[cfg(test)]
            played_samples: 0,
        }));
        let stream = match format {
            SampleFormat::F32 => stream::<f32>(&device, config, queue.clone())?,
            SampleFormat::F64 => stream::<f64>(&device, config, queue.clone())?,
            SampleFormat::I8 => stream::<i8>(&device, config, queue.clone())?,
            SampleFormat::I16 => stream::<i16>(&device, config, queue.clone())?,
            SampleFormat::I24 => stream::<cpal::I24>(&device, config, queue.clone())?,
            SampleFormat::I32 => stream::<i32>(&device, config, queue.clone())?,
            SampleFormat::I64 => stream::<i64>(&device, config, queue.clone())?,
            SampleFormat::U8 => stream::<u8>(&device, config, queue.clone())?,
            SampleFormat::U16 => stream::<u16>(&device, config, queue.clone())?,
            SampleFormat::U24 => stream::<cpal::U24>(&device, config, queue.clone())?,
            SampleFormat::U32 => stream::<u32>(&device, config, queue.clone())?,
            SampleFormat::U64 => stream::<u64>(&device, config, queue.clone())?,
            _ => return Err(format!("unsupported output sample format {format}").into()),
        };
        stream.play()?;
        Ok(Self {
            queue,
            _stream: stream,
        })
    }

    pub fn push(&self, samples: Vec<f32>) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        for sample in samples {
            if queue.samples.len() == MAX_QUEUED {
                queue.samples.pop_front();
            }
            queue.samples.push_back(sample);
        }
    }

    pub fn clear(&self) {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        queue.samples.clear();
        queue.phase = 0.0;
    }
}

fn stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    queue: Arc<Mutex<Queue>>,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = usize::from(config.channels);
    let step = SOURCE_RATE / f64::from(config.sample_rate);
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let Ok(mut queue) = queue.try_lock() else {
                data.fill(T::EQUILIBRIUM);
                return;
            };
            for frame in data.chunks_mut(channels) {
                let sample = queue.samples.front().copied().unwrap_or(0.0);
                frame.fill(T::from_sample(sample));
                queue.phase += step;
                while queue.phase >= 1.0 {
                    let consumed = queue.samples.pop_front();
                    #[cfg(test)]
                    if consumed.is_some_and(|sample| sample != 0.0) {
                        queue.played_samples += 1;
                    }
                    #[cfg(not(test))]
                    let _ = consumed;
                    queue.phase -= 1.0;
                }
            }
        },
        |error| eprintln!("dosa audio: {error}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a native audio output device"]
    fn native_output_consumes_a_bounded_guest_tone() {
        let audio = Audio::open().unwrap();
        let samples = (0..MAX_QUEUED * 2)
            .map(|index| if index % 100 < 50 { 0.05 } else { -0.05 })
            .collect();
        audio.push(samples);
        assert!(audio.queue.lock().unwrap().samples.len() <= MAX_QUEUED);
        for _ in 0..30 {
            if audio.queue.lock().unwrap().played_samples > 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("native output did not consume guest audio");
    }
}
