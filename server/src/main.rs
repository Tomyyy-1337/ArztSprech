use std::error::Error;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use arztsprech_core::protocol::Model;
use arztsprech_core::secure;
use arztsprech_core::transcription::Transcriber;
use arztsprech_server::auth::Users;
use arztsprech_server::server::{self, Transcribers};

fn env_path(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default))
}

fn main() -> Result<(), Box<dyn Error>> {
    let listen = std::env::var("ARZTSPRECH_LISTEN").unwrap_or_else(|_| "0.0.0.0:7878".to_string());
    let cert_path = env_path("ARZTSPRECH_CERT", "server/certs/server-cert.pem");
    let key_path = env_path("ARZTSPRECH_KEY", "server/certs/server-key.pem");
    let users_path = env_path("ARZTSPRECH_USERS", "server/certs/users.txt");

    if !cert_path.exists() || !key_path.exists() {
        if let Some(dir) = cert_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        secure::generate_self_signed(&cert_path, &key_path)?;
        println!("Neues Zertifikat erstellt: {} (diese Datei auf die Clients kopieren)", cert_path.display());
    }
    let tls = secure::server_config(&cert_path, &key_path)?;
    let users = Arc::new(Users::load(&users_path)?);

    println!("Lade {}", Model::Turbo.path());
    let turbo = Transcriber::load(Model::Turbo.path())?;
    println!("Lade {}", Model::GermanTurbo.path());
    let german_turbo = Transcriber::load(Model::GermanTurbo.path())?;
    let transcriber = Arc::new(Transcribers { turbo, german_turbo });

    let listener = TcpListener::bind(&listen)?;
    println!("Server lauscht auf {listen} (TLS, Anmeldung erforderlich)");
    for connection in listener.incoming() {
        match connection {
            Ok(connection) => {
                let transcriber = Arc::clone(&transcriber);
                let tls = Arc::clone(&tls);
                let users = Arc::clone(&users);
                std::thread::spawn(move || {
                    if let Err(err) = server::serve(connection, tls, &users, &transcriber) {
                        eprintln!("Verbindung beendet: {err}");
                    }
                });
            }
            Err(err) => eprintln!("Verbindungsfehler: {err}"),
        }
    }
    Ok(())
}
