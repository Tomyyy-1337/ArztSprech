use std::error::Error;
use std::sync::{Arc, Mutex};

use cpal::{FromSample, Sample};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[derive(Default)]
struct PlayState {
    /// Position in source frames.
    pos: f64,
    paused: bool,
    finished: bool,
}

/// Plays mono f32 samples on the default output device, with pause and seek.
pub struct Player {
    sample_rate: u32,
    len: usize,
    state: Arc<Mutex<PlayState>>,
    _stream: cpal::Stream,
}

impl Player {
    pub fn new(sample_rate: u32, samples: Vec<f32>) -> Result<Self, Box<dyn Error>> {
        let device = cpal::default_host().default_output_device().ok_or("Kein Ausgabegerät gefunden")?;
        let supported = device.default_output_config()?;
        let config = supported.config();
        let channels = config.channels as usize;
        // Source frames consumed per output frame.
        let step = sample_rate as f64 / config.sample_rate as f64;
        let len = samples.len();
        let state = Arc::new(Mutex::new(PlayState { paused: true, ..Default::default() }));
        let cb_state = state.clone();

        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => build_typed::<f32>(&device, config, samples, step, channels, cb_state)?,
            cpal::SampleFormat::I16 => build_typed::<i16>(&device, config, samples, step, channels, cb_state)?,
            cpal::SampleFormat::U16 => build_typed::<u16>(&device, config, samples, step, channels, cb_state)?,
            other => return Err(format!("Nicht unterstütztes Sample-Format: {other:?}").into()),
        };
        stream.play()?;
        Ok(Self { sample_rate, len, state, _stream: stream })
    }

    pub fn duration_secs(&self) -> f64 {
        self.len as f64 / self.sample_rate as f64
    }

    pub fn position_secs(&self) -> f64 {
        let state = self.lock();
        (state.pos / self.sample_rate as f64).min(self.duration_secs())
    }

    pub fn is_paused(&self) -> bool {
        self.lock().paused
    }

    pub fn is_finished(&self) -> bool {
        self.lock().finished
    }

    pub fn set_paused(&self, paused: bool) {
        let mut state = self.lock();
        // Pressing play at the end restarts from the beginning.
        if !paused && state.finished {
            state.pos = 0.0;
            state.finished = false;
        }
        state.paused = paused;
    }

    pub fn seek_secs(&self, secs: f64) {
        let mut state = self.lock();
        state.pos = (secs.max(0.0) * self.sample_rate as f64).min(self.len as f64);
        state.finished = false;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PlayState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn build_typed<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Vec<f32>,
    step: f64,
    channels: usize,
    state: Arc<Mutex<PlayState>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + FromSample<f32>,
{
    device.build_output_stream::<T, _, _>(
        config,
        move |data: &mut [T], _| {
            let mut state = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            for frame in data.chunks_mut(channels) {
                let index = state.pos as usize;
                let value = if state.paused || state.finished || index >= samples.len() {
                    0.0
                } else {
                    state.pos += step;
                    samples[index]
                };
                if !state.paused && !state.finished && state.pos as usize >= samples.len() {
                    state.finished = true;
                }
                let sample: T = value.to_sample::<T>();
                for out in frame {
                    *out = sample;
                }
            }
        },
        |err| eprintln!("Wiedergabefehler: {err}"),
        None,
    )
}
