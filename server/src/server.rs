use std::error::Error;
use std::net::TcpStream;
use std::sync::mpsc;

use arztsprech_core::audio::AudioSource;
use arztsprech_core::protocol::{self, Model};
use arztsprech_core::transcription::{Transcriber, run_transcriber};

/// Alle geladenen Modelle; jede Verbindung nutzt das vom Client gewählte.
pub struct Transcribers {
    pub turbo: Transcriber,
    pub german_turbo: Transcriber,
}

impl Transcribers {
    pub fn get(&self, model: Model) -> &Transcriber {
        match model {
            Model::Turbo => &self.turbo,
            Model::GermanTurbo => &self.german_turbo,
        }
    }
}

/// Bedient eine Verbindung: Audio kommt vom Client, Nachrichten gehen zurück.
pub fn serve(connection: TcpStream, transcribers: &Transcribers) -> Result<(), Box<dyn Error>> {
    let peer = connection.peer_addr()?;
    let mut reader = connection.try_clone()?;
    let mut writer = connection;
    let (sample_rate, model, client_prompt) = protocol::read_hello(&mut reader)?;
    let transcriber = transcribers.get(model);
    let prompt = (!client_prompt.trim().is_empty()).then_some(client_prompt.as_str());

    let (audio_tx, audio_rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(samples) = protocol::read_audio(&mut reader) {
            if audio_tx.send(samples).is_err() {
                break;
            }
        }
    });

    let source = AudioSource::from_channel(audio_rx, sample_rate, format!("Client {peer}"));
    run_transcriber(transcriber, source, &std::sync::atomic::AtomicBool::new(false), prompt, |msg| {
        let _ = protocol::write_msg(&mut writer, &msg);
    })
}
