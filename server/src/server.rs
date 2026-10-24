use std::error::Error;
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::mpsc;

use arztsprech_core::audio::AudioSource;
use arztsprech_core::protocol::{self, Model};
use arztsprech_core::secure;
use arztsprech_core::transcription::{Transcriber, run_transcriber};
use rustls::ServerConfig;

use crate::auth::Users;

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

/// Bedient eine Verbindung: TLS-Handshake, Anmeldung, dann Audio vom Client und Nachrichten zurück.
pub fn serve(
    connection: TcpStream,
    tls: Arc<ServerConfig>,
    users: &Users,
    transcribers: &Transcribers,
) -> Result<(), Box<dyn Error>> {
    let peer = connection.peer_addr()?;
    let (mut reader, mut writer) = secure::accept(connection, tls)?;

    let (username, password) = protocol::read_login(&mut reader)?;
    let authorized = users.verify(&username, &password);
    protocol::write_login_result(&mut writer, authorized)?;
    if !authorized {
        return Err(format!("Anmeldung fehlgeschlagen von {peer} (Benutzer \"{username}\")").into());
    }

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
