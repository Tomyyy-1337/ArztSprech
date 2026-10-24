use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arztsprech_core::audio;
use arztsprech_core::protocol::{self, Model, Msg};
use arztsprech_core::secure;

/// Serverzertifikat, das beim Bauen in den Client eingebettet wird. Dem Client wird nur dieses Zertifikat vertraut.
const SERVER_CERT: &[u8] = include_bytes!("../../server/certs/server-cert.pem");

/// Zugangsdaten für die Anmeldung am Server.
pub struct Login {
    pub username: String,
    pub password: String,
}

/// Sendet Mikrofon-Audio verschlüsselt an den Server und leitet dessen Nachrichten an `send` weiter.
/// Beendet die Verbindung, sobald `stop` gesetzt wird.
pub fn stream_microphone<F>(
    server: &str,
    model: Model,
    prompt: &str,
    mic: Option<String>,
    login: &Login,
    stop: Arc<AtomicBool>,
    send: F,
) -> Result<(), Box<dyn Error>>
where
    F: Fn(Msg) + Clone + Send + 'static,
{
    let source = audio::open_microphone(mic.as_deref())?;
    let tls = secure::client_config(SERVER_CERT)?;
    let (mut reader, mut writer) = secure::connect(server, tls)?;

    protocol::write_login(&mut writer, &login.username, &login.password)?;
    if !protocol::read_login_result(&mut reader)? {
        return Err("Anmeldung fehlgeschlagen: Benutzername oder Passwort falsch".into());
    }
    protocol::write_hello(&mut writer, source.sample_rate, model, prompt)?;

    let reply_send = send.clone();
    let reader_stop = stop.clone();
    std::thread::spawn(move || {
        while let Ok(msg) = protocol::read_msg(&mut reader) {
            reply_send(msg);
        }
        if !reader_stop.load(Ordering::SeqCst) {
            reply_send(Msg::Status("Verbindung zum Server getrennt".to_string()));
        }
    });

    send(Msg::Status(format!("Verbunden mit {server}")));
    while let Ok(samples) = source.rx.recv() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        protocol::write_audio(&mut writer, &samples)?;
    }
    Ok(())
}
