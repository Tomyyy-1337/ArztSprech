use std::error::Error;
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use arztsprech_core::audio;
use arztsprech_core::protocol::{self, Model, Msg};

/// Sendet Mikrofon-Audio an den Server und leitet dessen Nachrichten an `send` weiter.
/// Beendet die Verbindung, sobald `stop` gesetzt wird.
pub fn stream_microphone<F>(
    server: &str,
    model: Model,
    prompt: &str,
    mic: Option<String>,
    stop: Arc<AtomicBool>,
    send: F,
) -> Result<(), Box<dyn Error>>
where
    F: Fn(Msg) + Clone + Send + 'static,
{
    let source = audio::open_microphone(mic.as_deref())?;
    let mut writer = TcpStream::connect(server)?;
    protocol::write_hello(&mut writer, source.sample_rate, model, prompt)?;

    let mut reader = writer.try_clone()?;
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

    send(Msg::Status(format!("Verbunden mit {server} – {}", source.description)));
    while let Ok(samples) = source.rx.recv() {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        protocol::write_audio(&mut writer, &samples)?;
    }
    let _ = writer.shutdown(Shutdown::Both);
    Ok(())
}
