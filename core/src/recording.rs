use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};

/// Starts a WAV writer (16-bit mono) on a background thread.
/// Chunks sent to the returned sender are appended; the file is finalized once the sender is dropped.
pub fn start_writer(path: PathBuf, sample_rate: u32) -> io::Result<Sender<Vec<f32>>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(io::Error::other)?;
    let (tx, rx) = mpsc::channel::<Vec<f32>>();
    std::thread::spawn(move || {
        while let Ok(chunk) = rx.recv() {
            for sample in chunk {
                let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                if writer.write_sample(value).is_err() {
                    return;
                }
            }
        }
        let _ = writer.finalize();
    });
    Ok(tx)
}

/// Reads a WAV file written by [`start_writer`] and returns its sample rate and mono f32 samples.
pub fn read_wav(path: &Path) -> io::Result<(u32, Vec<f32>)> {
    let mut reader = hound::WavReader::open(path).map_err(io::Error::other)?;
    let sample_rate = reader.spec().sample_rate;
    let samples = reader
        .samples::<i16>()
        .map(|sample| sample.map(|s| s as f32 / i16::MAX as f32))
        .collect::<Result<Vec<f32>, _>>()
        .map_err(io::Error::other)?;
    Ok((sample_rate, samples))
}

/// Removes everything inside `dir` (the directory itself is recreated empty).
pub fn clear_dir(dir: &Path) -> io::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)
}
