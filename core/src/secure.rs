//! Verschlüsselte Verbindung (TLS) zwischen Client und Server.
//!
//! Die TLS-Sitzung gehört einem Pump-Thread. Lesen und Schreiben laufen über
//! Kanäle, sodass Client und Server wie bisher getrennte Reader/Writer nutzen können.

use std::error::Error;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, pem::PemObject};
use rustls::{
    ClientConfig, ClientConnection, ConnectionCommon, RootCertStore, ServerConfig, ServerConnection, SideData,
    StreamOwned,
};

/// Name im Zertifikat. Der Client prüft gegen diesen Namen und vertraut nur dem gepinnten Zertifikat.
pub const SERVER_NAME: &str = "arztsprech.local";
const POLL_INTERVAL: Duration = Duration::from_millis(10);
const ERROR_IO_PENDING: i32 = 997;

pub type Channel = (ChannelReader, ChannelWriter);

/// Erzeugt ein selbstsigniertes Zertifikat und speichert es als PEM.
pub fn generate_self_signed(cert_path: &Path, key_path: &Path) -> Result<(), Box<dyn Error>> {
    let certified = rcgen::generate_simple_self_signed(vec![SERVER_NAME.to_string()])?;
    std::fs::write(cert_path, certified.cert.pem())?;
    std::fs::write(key_path, certified.signing_key.serialize_pem())?;
    Ok(())
}

pub fn server_config(cert_path: &Path, key_path: &Path) -> Result<Arc<ServerConfig>, Box<dyn Error>> {
    let certs = CertificateDer::pem_file_iter(cert_path)?.collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_file(key_path)?;
    let config = ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?;
    Ok(Arc::new(config))
}

/// Der Client vertraut ausschließlich dem angegebenen (gepinnten) Serverzertifikat (PEM).
pub fn client_config(trusted_cert_pem: &[u8]) -> Result<Arc<ClientConfig>, Box<dyn Error>> {
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(trusted_cert_pem) {
        roots.add(cert?)?;
    }
    Ok(Arc::new(ClientConfig::builder().with_root_certificates(roots).with_no_client_auth()))
}

pub fn connect(server: &str, config: Arc<ClientConfig>) -> io::Result<Channel> {
    let tcp = TcpStream::connect(server)?;
    let name = ServerName::try_from(SERVER_NAME).map_err(io::Error::other)?;
    let conn = ClientConnection::new(config, name).map_err(io::Error::other)?;
    secure(conn, tcp)
}

pub fn accept(tcp: TcpStream, config: Arc<ServerConfig>) -> io::Result<Channel> {
    let conn = ServerConnection::new(config).map_err(io::Error::other)?;
    secure(conn, tcp)
}

fn secure<C, D>(conn: C, tcp: TcpStream) -> io::Result<Channel>
where
    C: DerefMut + Deref<Target = ConnectionCommon<D>> + Send + 'static,
    D: SideData + 'static,
{
    let mut tls = StreamOwned::new(conn, tcp);
    tls.conn.complete_io(&mut tls.sock)?;
    tls.sock.set_read_timeout(Some(POLL_INTERVAL))?;

    let (incoming_tx, incoming_rx) = mpsc::channel();
    let (outgoing_tx, outgoing_rx) = mpsc::channel();
    std::thread::spawn(move || pump(tls, outgoing_rx, incoming_tx));
    Ok((ChannelReader::new(incoming_rx), ChannelWriter { tx: outgoing_tx }))
}

/// Besitzt die TLS-Sitzung: schreibt ausgehende Daten und leitet eingehende weiter.
/// Endet, sobald die Verbindung geschlossen wurde oder der Writer verworfen wird.
fn pump<S: Read + Write>(mut stream: S, outgoing: mpsc::Receiver<Vec<u8>>, incoming: mpsc::Sender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        loop {
            match outgoing.try_recv() {
                Ok(bytes) => {
                    if stream.write_all(&bytes).and_then(|()| flush_retry(&mut stream)).is_err() {
                        return;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return,
            }
        }
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if incoming.send(buf[..n].to_vec()).is_err() {
                    return;
                }
            }
            Err(err) if is_transient(&err) => std::thread::sleep(Duration::from_millis(1)),
            Err(_) => return,
        }
    }
}

/// Wiederholt `flush`, solange der Fehler nur vorübergehend ist; ein erneutes Schreiben der Daten entfällt.
fn flush_retry<S: Write>(stream: &mut S) -> io::Result<()> {
    loop {
        match stream.flush() {
            Ok(()) => return Ok(()),
            Err(err) if is_transient(&err) => std::thread::sleep(Duration::from_millis(1)),
            Err(err) => return Err(err),
        }
    }
}

/// Socket-Timeouts zeigt Windows teils als ERROR_IO_PENDING (997) statt als TimedOut/WouldBlock.
fn is_transient(err: &io::Error) -> bool {
    matches!(err.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted)
        || err.raw_os_error() == Some(ERROR_IO_PENDING)
}


/// Entschlüsselte eingehende Daten als `Read`. Liefert `Ok(0)`, wenn die Verbindung endet.
pub struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    pos: usize,
}

impl ChannelReader {
    fn new(rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self { rx, pending: Vec::new(), pos: 0 }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos == self.pending.len() {
            match self.rx.recv() {
                Ok(bytes) => {
                    self.pending = bytes;
                    self.pos = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.pending.len() - self.pos);
        out[..n].copy_from_slice(&self.pending[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Ausgehende Daten als `Write`; sie werden verschlüsselt vom Pump-Thread gesendet.
pub struct ChannelWriter {
    tx: mpsc::Sender<Vec<u8>>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tx
            .send(buf.to_vec())
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "Verbindung geschlossen"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::protocol::{self, Msg};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("arztsprech-secure-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cert_pair(dir: &Path) -> (PathBuf, PathBuf) {
        let cert = dir.join("cert.pem");
        let key = dir.join("key.pem");
        generate_self_signed(&cert, &key).unwrap();
        (cert, key)
    }

    #[test]
    fn login_and_reply_over_tls() {
        let dir = temp_dir("ok");
        let (cert, key) = cert_pair(&dir);
        let server_tls = server_config(&cert, &key).unwrap();
        let client_tls = client_config(&std::fs::read(&cert).unwrap()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let (mut reader, mut writer) = accept(tcp, server_tls).unwrap();
            let login = protocol::read_login(&mut reader).unwrap();
            protocol::write_login_result(&mut writer, login == ("arzt".to_string(), "geheim".to_string())).unwrap();
            protocol::write_msg(&mut writer, &Msg::Final("hallo".to_string())).unwrap();
        });

        let (mut reader, mut writer) = connect(&addr, client_tls).unwrap();
        protocol::write_login(&mut writer, "arzt", "geheim").unwrap();
        assert!(protocol::read_login_result(&mut reader).unwrap());
        match protocol::read_msg(&mut reader).unwrap() {
            Msg::Final(text) => assert_eq!(text, "hallo"),
            _ => panic!("unerwartete Nachricht"),
        }
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sustained_two_way_traffic_stays_open() {
        let dir = temp_dir("stress");
        let (cert, key) = cert_pair(&dir);
        let server_tls = server_config(&cert, &key).unwrap();
        let client_tls = client_config(&std::fs::read(&cert).unwrap()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let (mut reader, mut writer) = accept(tcp, server_tls).unwrap();
            protocol::read_login(&mut reader).unwrap();
            protocol::write_login_result(&mut writer, true).unwrap();
            let done = Arc::new(AtomicBool::new(false));
            let done_flag = done.clone();
            let audio_reader = std::thread::spawn(move || {
                let mut count = 0usize;
                while protocol::read_audio(&mut reader).is_ok() {
                    count += 1;
                }
                done_flag.store(true, Ordering::SeqCst);
                count
            });
            let mut write_err = None;
            let mut i = 0;
            while !done.load(Ordering::SeqCst) {
                if let Err(err) = protocol::write_msg(&mut writer, &Msg::Partial(format!("text {i}"))) {
                    write_err = Some((i, err));
                    break;
                }
                i += 1;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            drop(writer);
            (write_err.map(|(i, e)| format!("server write failed at {i}: {e}")), audio_reader.join().unwrap())
        });

        let (mut reader, mut writer) = connect(&addr, client_tls).unwrap();
        protocol::write_login(&mut writer, "arzt", "geheim").unwrap();
        assert!(protocol::read_login_result(&mut reader).unwrap());
        let msg_reader = std::thread::spawn(move || {
            let mut count = 0usize;
            while protocol::read_msg(&mut reader).is_ok() {
                count += 1;
            }
            count
        });
        let samples = vec![0.1f32; 480];
        let mut audio_err = None;
        for i in 0..300 {
            if let Err(err) = protocol::write_audio(&mut writer, &samples) {
                audio_err = Some(format!("client audio write failed at {i}: {err}"));
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        drop(writer);

        let (server_err, audio_count) = server.join().unwrap();
        let msg_count = msg_reader.join().unwrap();
        assert!(server_err.is_none(), "{server_err:?}");
        assert!(audio_err.is_none(), "{audio_err:?}");
        assert!(msg_count > 0, "keine Nachrichten beim Client");
        assert_eq!(audio_count, 300, "Audio beim Server");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Liefert nur jeden zweiten Lesevorgang Daten, sonst ERROR_IO_PENDING (997).
    struct FlakyStream {
        input: std::io::Cursor<Vec<u8>>,
        reads: usize,
        flushes: usize,
    }

    impl Read for FlakyStream {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.reads += 1;
            if self.reads.is_multiple_of(2) {
                return Err(io::Error::from_raw_os_error(ERROR_IO_PENDING));
            }
            let n = buf.len().min(3);
            self.input.read(&mut buf[..n])
        }
    }

    impl Write for FlakyStream {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.flushes == 1 {
                return Err(io::Error::from_raw_os_error(ERROR_IO_PENDING));
            }
            Ok(())
        }
    }

    #[test]
    fn pump_survives_transient_windows_io_pending() {
        let stream = FlakyStream { input: std::io::Cursor::new(b"hallo welt".to_vec()), reads: 0, flushes: 0 };
        let (out_tx, out_rx) = mpsc::channel();
        let (in_tx, in_rx) = mpsc::channel();
        out_tx.send(b"ping".to_vec()).unwrap();

        let handle = std::thread::spawn(move || {
            pump(stream, out_rx, in_tx);
        });
        let received: Vec<u8> = in_rx.iter().flatten().collect();
        handle.join().unwrap();
        drop(out_tx);
        assert_eq!(received, b"hallo welt");
    }

    #[test]
    fn untrusted_server_certificate_is_rejected() {
        let dir = temp_dir("untrusted");
        let (cert, key) = cert_pair(&dir);
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let (other_cert, _) = cert_pair(&other);
        let server_tls = server_config(&cert, &key).unwrap();
        let client_tls = client_config(&std::fs::read(&other_cert).unwrap()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            let _ = accept(tcp, server_tls);
        });

        assert!(connect(&addr, client_tls).is_err());
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
