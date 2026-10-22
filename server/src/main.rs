use std::error::Error;
use std::net::TcpListener;
use std::sync::Arc;

use arztsprech_core::transcription::{DEFAULT_MODEL, Transcriber};
use arztsprech_server::server;

fn main() -> Result<(), Box<dyn Error>> {
    let listen = std::env::var("ARZTSPRECH_LISTEN").unwrap_or_else(|_| "0.0.0.0:7878".to_string());
    let model = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let transcriber = Arc::new(Transcriber::load(&model)?);

    let listener = TcpListener::bind(&listen)?;
    println!("Server lauscht auf {listen}");
    for connection in listener.incoming() {
        match connection {
            Ok(connection) => {
                let transcriber = Arc::clone(&transcriber);
                std::thread::spawn(move || {
                    if let Err(err) = server::serve(connection, &transcriber) {
                        eprintln!("Verbindung beendet: {err}");
                    }
                });
            }
            Err(err) => eprintln!("Verbindungsfehler: {err}"),
        }
    }
    Ok(())
}
