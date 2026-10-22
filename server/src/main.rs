use std::error::Error;
use std::net::TcpListener;
use std::sync::Arc;

use arztsprech_core::protocol::Model;
use arztsprech_core::transcription::Transcriber;
use arztsprech_server::server::{self, Transcribers};

fn main() -> Result<(), Box<dyn Error>> {
    let listen = std::env::var("ARZTSPRECH_LISTEN").unwrap_or_else(|_| "0.0.0.0:7878".to_string());
    println!("Lade {}", Model::Turbo.path());
    let turbo = Transcriber::load(Model::Turbo.path())?;
    println!("Lade {}", Model::GermanTurbo.path());
    let german_turbo = Transcriber::load(Model::GermanTurbo.path())?;
    let transcriber = Arc::new(Transcribers { turbo, german_turbo });

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
