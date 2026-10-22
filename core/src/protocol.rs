use std::io::{self, Read, Write};

pub enum Msg {
    Status(String),
    Partial(String),
    Final(String),
}

const MAX_AUDIO_SAMPLES: usize = 16_000 * 10;
const MAX_TEXT_BYTES: usize = 1 << 20;

pub fn write_hello(w: &mut impl Write, sample_rate: u32) -> io::Result<()> {
    w.write_all(&sample_rate.to_le_bytes())
}

pub fn read_hello(r: &mut impl Read) -> io::Result<u32> {
    read_u32(r)
}

pub fn write_audio(w: &mut impl Write, samples: &[f32]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(4 + samples.len() * 2);
    buf.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    for sample in samples {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        buf.extend_from_slice(&value.to_le_bytes());
    }
    w.write_all(&buf)
}

pub fn read_audio(r: &mut impl Read) -> io::Result<Vec<f32>> {
    let len = read_u32(r)? as usize;
    if len > MAX_AUDIO_SAMPLES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Audio-Block zu groß"));
    }
    let mut buf = vec![0u8; len * 2];
    r.read_exact(&mut buf)?;
    Ok(buf
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / i16::MAX as f32)
        .collect())
}

pub fn write_msg(w: &mut impl Write, msg: &Msg) -> io::Result<()> {
    let (tag, text) = match msg {
        Msg::Status(text) => (0u8, text),
        Msg::Partial(text) => (1u8, text),
        Msg::Final(text) => (2u8, text),
    };
    let bytes = text.as_bytes();
    let mut buf = Vec::with_capacity(5 + bytes.len());
    buf.push(tag);
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    w.write_all(&buf)
}

pub fn read_msg(r: &mut impl Read) -> io::Result<Msg> {
    let mut tag = [0u8; 1];
    r.read_exact(&mut tag)?;
    let len = read_u32(r)? as usize;
    if len > MAX_TEXT_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Nachricht zu groß"));
    }
    let mut bytes = vec![0u8; len];
    r.read_exact(&mut bytes)?;
    let text = String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    match tag[0] {
        0 => Ok(Msg::Status(text)),
        1 => Ok(Msg::Partial(text)),
        2 => Ok(Msg::Final(text)),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Unbekannter Nachrichtentyp")),
    }
}

fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}
