use std::error::Error;
use std::net::TcpStream;

use arztsprech_core::audio;
use arztsprech_core::protocol::{self, Msg};

/// Sendet Mikrofon-Audio an den Server und leitet dessen Nachrichten an `send` weiter.
pub fn stream_microphone<F>(server: &str, send: F) -> Result<(), Box<dyn Error>>
where
    F: Fn(Msg) + Clone + Send + 'static,
{
    let source = audio::open_microphone()?;
    let mut writer = TcpStream::connect(server)?;
    protocol::write_hello(&mut writer, source.sample_rate)?;

    let mut reader = writer.try_clone()?;
    let reply_send = send.clone();
    std::thread::spawn(move || {
        while let Ok(msg) = protocol::read_msg(&mut reader) {
            reply_send(msg);
        }
        reply_send(Msg::Status("Verbindung zum Server getrennt".to_string()));
    });

    send(Msg::Status(format!("Verbunden mit {server} – {}", source.description)));
    while let Ok(samples) = source.rx.recv() {
        protocol::write_audio(&mut writer, &samples)?;
    }
    Ok(())
}
