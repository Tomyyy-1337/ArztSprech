use std::error::Error;
use std::net::TcpStream;
use std::sync::mpsc;

use arztsprech_core::audio::AudioSource;
use arztsprech_core::protocol;
use arztsprech_core::transcription::{Transcriber, run_transcriber};

/// Bedient eine Verbindung: Audio kommt vom Client, Nachrichten gehen zurück.
pub fn serve(connection: TcpStream, transcriber: &Transcriber) -> Result<(), Box<dyn Error>> {
    let peer = connection.peer_addr()?;
    let mut reader = connection.try_clone()?;
    let mut writer = connection;
    let sample_rate = protocol::read_hello(&mut reader)?;

    let (audio_tx, audio_rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(samples) = protocol::read_audio(&mut reader) {
            if audio_tx.send(samples).is_err() {
                break;
            }
        }
    });

    let source = AudioSource::from_channel(audio_rx, sample_rate, format!("Client {peer}"));
    run_transcriber(transcriber, source, |msg| {
        let _ = protocol::write_msg(&mut writer, &msg);
    })
}
