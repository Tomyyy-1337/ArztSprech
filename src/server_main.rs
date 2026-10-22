use std::error::Error;
use std::net::TcpListener;

use arztsprech::server;
use arztsprech::transcription::{DEFAULT_MODEL, Transcriber};

fn main() -> Result<(), Box<dyn Error>> {
    let listen = std::env::var("ARZTSPRECH_LISTEN").unwrap_or_else(|_| "0.0.0.0:7878".to_string());
    let model = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let transcriber = Transcriber::load(&model)?;

    let listener = TcpListener::bind(&listen)?;
    println!("Server lauscht auf {listen}");
    for connection in listener.incoming() {
        match connection {
            Ok(connection) => {
                if let Err(err) = server::serve(connection, &transcriber) {
                    eprintln!("Verbindung beendet: {err}");
                }
            }
            Err(err) => eprintln!("Verbindungsfehler: {err}"),
        }
    }
    Ok(())
}
