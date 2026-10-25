use std::error::Error;
use std::sync::mpsc::{self, Receiver, Sender};

use cpal::FromSample;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Mono-f32-Audio aus einer beliebigen Quelle; `_keep_alive` hält z. B. den Mikrofon-Stream am Leben.
pub struct AudioSource {
    pub rx: Receiver<Vec<f32>>,
    pub sample_rate: u32,
    pub description: String,
    _keep_alive: Option<cpal::Stream>,
}

impl AudioSource {
    pub fn from_channel(rx: Receiver<Vec<f32>>, sample_rate: u32, description: String) -> Self {
        Self { rx, sample_rate, description, _keep_alive: None }
    }
}

pub fn input_device_names() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|devices| {
            devices
                .filter_map(|device| device.description().ok().map(|desc| desc.name().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn find_input_device(name: Option<&str>) -> Result<cpal::Device, Box<dyn Error>> {
    let host = cpal::default_host();
    let device = match name {
        Some(name) => host
            .input_devices()?
            .find(|device| device.description().is_ok_and(|desc| desc.name() == name))
            .ok_or_else(|| format!("Mikrofon nicht gefunden: {name}"))?,
        None => host.default_input_device().ok_or("Kein Mikrofon gefunden")?,
    };
    Ok(device)
}

/// Sample rate the named input device (or the system default) delivers.
pub fn input_sample_rate(name: Option<&str>) -> Result<u32, Box<dyn Error>> {
    Ok(find_input_device(name)?.default_input_config()?.sample_rate())
}

/// Opens the named input device, or the system default when `name` is `None`.
/// Every mono chunk is also copied to `tap` when given, e.g. for recording.
pub fn open_microphone(name: Option<&str>, tap: Option<Sender<Vec<f32>>>) -> Result<AudioSource, Box<dyn Error>> {
    let device = find_input_device(name)?;
    let supported = device.default_input_config()?;
    let config = supported.config();

    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    let stream = build_stream(&device, &supported, config, tx, tap)?;
    stream.play()?;

    Ok(AudioSource {
        rx,
        sample_rate: config.sample_rate,
        description: format!("Mikrofon: {}", device.description()?.name()),
        _keep_alive: Some(stream),
    })
}

fn build_stream(
    device: &cpal::Device,
    supported: &cpal::SupportedStreamConfig,
    config: cpal::StreamConfig,
    tx: Sender<Vec<f32>>,
    tap: Option<Sender<Vec<f32>>>,
) -> Result<cpal::Stream, Box<dyn Error>> {
    let stream = match supported.sample_format() {
        cpal::SampleFormat::F32 => build_typed::<f32>(device, config, tx, tap)?,
        cpal::SampleFormat::I16 => build_typed::<i16>(device, config, tx, tap)?,
        cpal::SampleFormat::U16 => build_typed::<u16>(device, config, tx, tap)?,
        other => return Err(format!("Nicht unterstütztes Sample-Format: {other:?}").into()),
    };
    Ok(stream)
}

/// Sendet Audio als Mono-f32, egal in welchem Format das Mikrofon liefert.
fn build_typed<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    tx: Sender<Vec<f32>>,
    tap: Option<Sender<Vec<f32>>>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    let channels = config.channels as usize;
    device.build_input_stream::<T, _, _>(
        config,
        move |data: &[T], _| {
            let mono: Vec<f32> = data
                .chunks(channels)
                .map(|frame| frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32)
                .collect();
            if let Some(tap) = &tap {
                let _ = tap.send(mono.clone());
            }
            let _ = tx.send(mono);
        },
        |err| eprintln!("Audiofehler: {err}"),
        None,
    )
}
