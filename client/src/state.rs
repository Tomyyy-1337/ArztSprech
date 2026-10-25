use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use enigo::{Enigo, Keyboard, Settings};
use iced::widget::text_editor;
use iced::{Size, Subscription, Task, Theme, window};

use arztsprech_core::audio;
use arztsprech_core::playback::Player;
use arztsprech_core::protocol::{Model, Msg};
use arztsprech_core::recording;
#[cfg(feature = "remote")]
use crate::client;
#[cfg(not(feature = "remote"))]
use arztsprech_core::transcription::{Transcriber, run_transcriber};
#[cfg(not(feature = "remote"))]
use std::error::Error;

use super::layout::app_theme;

#[cfg(feature = "remote")]
const DEFAULT_SERVER: &str = "127.0.0.1:7878";
const DEFAULT_PROMPT: &str = "Deutschsprachiges gastroenterologisches Diktat. Fachbegriffe: ";
pub(super) const SYSTEM_MIC: &str = "Systemstandard";
/// Breite des Aufnahmen-Panels, um die sich das Fenster beim Ein- und Ausklappen ändert.
pub(super) const PANEL_WIDTH: f32 = 320.0;

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Copy,
    Clear,
    ToggleRecording(bool),
    TogglePanel,
    OpenAudioFolder,
    SelectClip(u64),
    HoverClip(Option<u64>),
    TogglePlayback,
    Seek(f64),
    AutoInsertToggled(bool),
    LiveEditToggled(bool),
    ToggleSettings,
    ThemeSelected(bool),
    WindowResized(Size),
    ServerInput(String),
    ApplyServer,
    #[cfg(feature = "remote")]
    UsernameInput(String),
    #[cfg(feature = "remote")]
    ProductKeyInput(String),
    #[cfg(feature = "remote")]
    ApplyCredentials,
    ModelSelected(Model),
    MicSelected(String),
    PromptEdit(text_editor::Action),
    ApplyPrompt,
}

/// Eine Aufnahme (Start bis Stopp des Backends) mit ihrer WAV-Datei.
pub(super) struct Clip {
    pub(super) id: u64,
    /// Lokale Startzeit als `HH:MM:SS`.
    pub(super) clock: String,
    /// Start relativ zum Programmstart.
    pub(super) start: Duration,
    /// Ende relativ zum Programmstart; `None`, solange die Aufnahme läuft.
    pub(super) end: Option<Duration>,
    pub(super) path: PathBuf,
}

/// Finalisierter Transkripttext mit dem Zeitpunkt, zu dem er eintraf (relativ zum Programmstart).
pub(super) struct Segment {
    pub(super) range: Range<usize>,
    pub(super) at: Duration,
}

pub struct App {
    pub(super) rx: Receiver<Msg>,
    pub(super) tx: Sender<Msg>,
    pub(super) started: Instant,
    pub(super) text: String,
    pub(super) segments: Vec<Segment>,
    pub(super) clips: Vec<Clip>,
    pub(super) next_clip_id: u64,
    pub(super) selected_clip: Option<u64>,
    pub(super) hovered_clip: Option<u64>,
    pub(super) player: Option<Player>,
    pub(super) panel_open: bool,
    pub(super) partial: String,
    pub(super) status: String,
    pub(super) recording: bool,
    pub(super) auto_insert: bool,
    pub(super) live_edit: bool,
    pub(super) typed_partial: String,
    pub(super) enigo: Option<Enigo>,
    pub(super) server: String,
    pub(super) server_input: String,
    #[cfg(feature = "remote")]
    pub(super) username_input: String,
    #[cfg(feature = "remote")]
    pub(super) product_key_input: String,
    pub(super) model: Model,
    pub(super) prompt: String,
    pub(super) prompt_input: text_editor::Content,
    pub(super) mic: Option<String>,
    pub(super) mic_options: Vec<String>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) show_settings: bool,
    pub(super) window_size: Size,
    pub(super) backend: Option<std::thread::JoinHandle<()>>,
    pub(super) theme: Theme,
    pub(super) dark: bool,
}

pub fn boot() -> App {
    let (tx, rx) = mpsc::channel();
    let server = default_server();
    #[cfg(feature = "remote")]
    let (username, product_key) = load_credentials();
    let model = default_model();
    let prompt = load_prompt();
    let mic = load_mic();
    let dark = load_dark();
    let auto_insert = load_auto_insert();
    let live_edit = load_live_edit();
    let stop = Arc::new(AtomicBool::new(false));
    if let Some(dir) = audio_dir() {
        let _ = recording::clear_dir(&dir);
    }
    #[cfg(not(feature = "remote"))]
    preload_model(model, tx.clone());
    App {
        rx,
        tx,
        started: Instant::now(),
        text: String::new(),
        segments: Vec::new(),
        clips: Vec::new(),
        next_clip_id: 0,
        selected_clip: None,
        hovered_clip: None,
        player: None,
        panel_open: false,
        partial: String::new(),
        status: "Mikrofon aus".to_string(),
        recording: false,
        auto_insert,
        live_edit,
        typed_partial: String::new(),
        enigo: None,
        server: server.clone(),
        server_input: server,
        #[cfg(feature = "remote")]
        username_input: username,
        #[cfg(feature = "remote")]
        product_key_input: product_key,
        model,
        prompt_input: text_editor::Content::with_text(&prompt),
        prompt,
        mic,
        mic_options: audio::input_device_names(),
        stop,
        show_settings: false,
        window_size: Size::new(500.0, 180.0),
        backend: None,
        theme: app_theme(dark),
        dark,
    }
}

#[cfg(feature = "remote")]
fn default_server() -> String {
    load_server()
        .or_else(|| std::env::var("ARZTSPRECH_SERVER").ok())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

#[cfg(not(feature = "remote"))]
fn default_server() -> String {
    String::new()
}

/// Speicherort: `%APPDATA%\arztsprech\<file>`.
fn config_path(file: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|dir| std::path::PathBuf::from(dir).join("arztsprech").join(file))
}

#[cfg(feature = "remote")]
fn load_server() -> Option<String> {
    let server = std::fs::read_to_string(config_path("server.txt")?).ok()?;
    let server = server.trim().to_string();
    (!server.is_empty()).then_some(server)
}

fn save_server(server: &str) {
    write_config("server.txt", server);
}

fn load_prompt() -> String {
    config_path("prompt.txt")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_else(|| DEFAULT_PROMPT.to_string())
}

fn save_prompt(prompt: &str) {
    write_config("prompt.txt", prompt);
}

fn default_model() -> Model {
    config_path("model.txt")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|key| model_from_key(key.trim()))
        .unwrap_or(Model::GermanTurbo)
}

fn save_model(model: Model) {
    write_config("model.txt", model_key(model));
}

fn load_mic() -> Option<String> {
    config_path("mic.txt")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
}

fn save_mic(mic: Option<&str>) {
    write_config("mic.txt", mic.unwrap_or(""));
}

fn load_dark() -> bool {
    config_path("theme.txt")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|mode| mode.trim() != "light")
        .unwrap_or(true)
}

fn save_dark(dark: bool) {
    write_config("theme.txt", if dark { "dark" } else { "light" });
}

fn load_auto_insert() -> bool {
    config_path("auto_insert.txt")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|mode| mode.trim() == "on")
}

fn save_auto_insert(on: bool) {
    write_config("auto_insert.txt", if on { "on" } else { "off" });
}

fn load_live_edit() -> bool {
    config_path("live_edit.txt")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|mode| mode.trim() == "on")
}

fn save_live_edit(on: bool) {
    write_config("live_edit.txt", if on { "on" } else { "off" });
}

/// Speicherort der Aufnahmen: `%APPDATA%\arztsprech\audio`. Wird beim Start geleert.
fn audio_dir() -> Option<PathBuf> {
    config_path("audio")
}

fn write_config(file: &str, content: &str) {
    if let Some(path) = config_path(file) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, content);
    }
}

fn model_key(model: Model) -> &'static str {
    match model {
        Model::Turbo => "turbo",
        Model::GermanTurbo => "german-turbo",
    }
}

fn model_from_key(key: &str) -> Option<Model> {
    Model::ALL.into_iter().find(|&model| model_key(model) == key)
}

pub(super) fn model_label(model: Model) -> &'static str {
    match model {
        Model::Turbo => "large-v3-turbo",
        Model::GermanTurbo => "large-v3-turbo-german",
    }
}

fn make_send(tx: Sender<Msg>) -> impl Fn(Msg) + Clone + Send + 'static {
    move |msg: Msg| {
        let _ = tx.send(msg);
    }
}

#[cfg(feature = "remote")]
fn spawn_backend(server: &str, model: Model, prompt: &str, mic: Option<String>, tap: Option<Sender<Vec<f32>>>, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) -> std::thread::JoinHandle<()> {
    let server = server.to_string();
    let prompt = prompt.to_string();
    std::thread::spawn(move || {
        let gate = stop.clone();
        let send = move |msg: Msg| {
            if !gate.load(Ordering::SeqCst) {
                send(msg);
            }
        };
        let error_send = send.clone();
        let result = load_login().and_then(|login| {
            client::stream_microphone(&server, model, &prompt, mic, tap, &login, stop, send).map_err(|err| err.to_string())
        });
        if let Err(err) = result {
            error_send(Msg::Status(format!("Fehler: {err}")));
        }
    })
}

/// Liest `credentials.txt`: Zeile 1 Benutzername, Zeile 2 Passwort. Fehlende Werte sind leer.
#[cfg(feature = "remote")]
fn load_credentials() -> (String, String) {
    let credentials = config_path("credentials.txt")
        .and_then(|path| std::fs::read_to_string(path).ok())
        .unwrap_or_default();
    let mut lines = credentials.lines().map(|line| line.trim().to_string());
    let username = lines.next().unwrap_or_default();
    let password = lines.next().unwrap_or_default();
    (username, password)
}

#[cfg(feature = "remote")]
fn save_credentials(username: &str, password: &str) {
    write_config("credentials.txt", &format!("{username}\n{password}\n"));
}

/// Liest die Zugangsdaten aus `credentials.txt`. Das Serverzertifikat ist in den Client eingebettet.
#[cfg(feature = "remote")]
fn load_login() -> Result<client::Login, String> {
    let (username, password) = load_credentials();
    if username.is_empty() || password.is_empty() {
        return Err("credentials.txt: Benutzername (Zeile 1) und Produktschlüssel (Zeile 2) angeben".to_string());
    }
    Ok(client::Login { username, password })
}

#[cfg(not(feature = "remote"))]
fn spawn_backend(_server: &str, model: Model, prompt: &str, mic: Option<String>, tap: Option<Sender<Vec<f32>>>, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) -> std::thread::JoinHandle<()> {
    let prompt = prompt.to_string();
    std::thread::spawn(move || {
        let error_send = send.clone();
        if let Err(err) = run_standalone(model, &prompt, mic, tap, stop, send) {
            error_send(Msg::Status(format!("Fehler: {err}")));
        }
    })
}

#[cfg(not(feature = "remote"))]
fn join_backend(handle: Option<std::thread::JoinHandle<()>>) {
    if let Some(handle) = handle {
        let _ = handle.join();
    }
}

#[cfg(feature = "remote")]
fn join_backend(_handle: Option<std::thread::JoinHandle<()>>) {}

/// Keeps only the most recently used model loaded. The lock also makes a recording
/// wait for an in-progress preload instead of loading the same model twice.
#[cfg(not(feature = "remote"))]
fn cached_transcriber(model: Model) -> Result<Arc<Transcriber>, Box<dyn Error>> {
    static CACHE: std::sync::Mutex<Option<(Model, Arc<Transcriber>)>> = std::sync::Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((loaded, transcriber)) = cache.as_ref() {
        if *loaded == model {
            return Ok(transcriber.clone());
        }
    }
    // Free the previous model before loading the new one to avoid holding both in GPU memory.
    *cache = None;
    let transcriber = Arc::new(Transcriber::load(model.path())?);
    *cache = Some((model, transcriber.clone()));
    Ok(transcriber)
}

/// Lädt das gewählte Modell im Hintergrund, damit es beim ersten Diktat bereitsteht.
#[cfg(not(feature = "remote"))]
fn preload_model(model: Model, tx: Sender<Msg>) {
    std::thread::spawn(move || {
        let _ = tx.send(Msg::Status(format!("Lade Modell {} ...", model_label(model))));
        let status = match cached_transcriber(model) {
            Ok(_) => "Mikrofon aus".to_string(),
            Err(err) => format!("Fehler: {err}"),
        };
        let _ = tx.send(Msg::Status(status));
    });
}

#[cfg(not(feature = "remote"))]
fn run_standalone(model: Model, prompt: &str, mic: Option<String>, tap: Option<Sender<Vec<f32>>>, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Send + 'static) -> Result<(), Box<dyn Error>> {
    let gate = stop.clone();
    let send = move |msg: Msg| {
        if !gate.load(Ordering::SeqCst) {
            send(msg);
        }
    };
    send(Msg::Status(format!("Lade Modell {} ...", model_label(model))));
    let transcriber = cached_transcriber(model)?;
    let prompt = (!prompt.trim().is_empty()).then_some(prompt);
    run_transcriber(&transcriber, audio::open_microphone(mic.as_deref(), tap)?, &stop, prompt, send)
}

impl App {
    pub fn theme(&self) -> Theme {
        self.theme.clone()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
        ])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => self.drain_backend(),
            Message::Copy => return iced::clipboard::write(self.text.clone()),
            Message::Clear => {
                self.text.clear();
                self.segments.clear();
                self.partial.clear();
            }
            Message::TogglePanel => {
                self.panel_open = !self.panel_open;
                let size = self.window_target();
                return window::latest().and_then(move |id| window::resize(id, size));
            }
            Message::SelectClip(id) => self.select_clip(id),
            Message::OpenAudioFolder => self.open_audio_folder(),
            Message::HoverClip(id) => self.hovered_clip = id,
            Message::TogglePlayback => {
                if let Some(player) = self.player.as_ref() {
                    player.set_paused(!(player.is_paused() || player.is_finished()));
                }
            }
            Message::Seek(secs) => {
                if let Some(player) = self.player.as_ref() {
                    player.seek_secs(secs);
                }
            }
            Message::ToggleRecording(recording) => {
                self.recording = recording;
                if recording {
                    self.partial.clear();
                    self.typed_partial.clear();
                    self.start_backend();
                } else {
                    self.stop_backend();
                    self.partial.clear();
                    self.erase_typed_partial();
                    self.status = "Mikrofon aus".to_string();
                }
            }
            Message::AutoInsertToggled(on) => {
                self.auto_insert = on;
                save_auto_insert(on);
                if !on {
                    self.erase_typed_partial();
                }
            }
            Message::LiveEditToggled(on) => {
                self.live_edit = on;
                save_live_edit(on);
            }
            Message::ToggleSettings => {
                self.show_settings = !self.show_settings;
                if self.show_settings {
                    self.mic_options = audio::input_device_names();
                }
                let size = self.window_target();
                return window::latest().and_then(move |id| window::resize(id, size));
            }
            Message::WindowResized(size) => {
                if !self.show_settings {
                    self.window_size = if self.panel_open { Size::new(size.width - PANEL_WIDTH, size.height) } else { size };
                }
            }
            Message::ThemeSelected(dark) => {
                if dark != self.dark {
                    self.dark = dark;
                    self.theme = app_theme(dark);
                    save_dark(dark);
                }
            }
            Message::ServerInput(server) => self.server_input = server,
            #[cfg(feature = "remote")]
            Message::UsernameInput(username) => self.username_input = username,
            #[cfg(feature = "remote")]
            Message::ProductKeyInput(product_key) => self.product_key_input = product_key,
            #[cfg(feature = "remote")]
            Message::ApplyCredentials => {
                let username = self.username_input.trim().to_string();
                let product_key = self.product_key_input.trim().to_string();
                if username.is_empty() || product_key.is_empty() {
                    self.status = "Benutzername und Produktschlüssel eingeben".to_string();
                } else {
                    save_credentials(&username, &product_key);
                    self.status = "Zugangsdaten gespeichert".to_string();
                    let server = self.server.clone();
                    self.restart(server);
                }
            }
            Message::ApplyServer => {
                let server = self.server_input.trim().to_string();
                if !server.is_empty() {
                    save_server(&server);
                    self.restart(server);
                }
            }
            Message::ModelSelected(model) => {
                if model != self.model {
                    self.model = model;
                    save_model(model);
                    #[cfg(not(feature = "remote"))]
                    if !self.recording {
                        preload_model(model, self.tx.clone());
                    }
                    let server = self.server.clone();
                    self.restart(server);
                }
            }
            Message::MicSelected(name) => {
                let mic = (name != SYSTEM_MIC).then_some(name);
                if mic != self.mic {
                    save_mic(mic.as_deref());
                    self.mic = mic;
                    let server = self.server.clone();
                    self.restart(server);
                }
            }
            Message::PromptEdit(action) => self.prompt_input.perform(action),
            Message::ApplyPrompt => {
                self.prompt = self.prompt_input.text().trim().to_string();
                save_prompt(&self.prompt);
                let server = self.server.clone();
                self.restart(server);
            }
        }
        Task::none()
    }

    fn drain_backend(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Status(status) => self.status = status,
                Msg::Partial(text) => {
                    if self.recording && self.auto_insert && self.live_edit {
                        self.update_typed_partial(&text);
                    }
                    self.partial = text;
                }
                Msg::Final(text) => {
                    self.partial.clear();
                    self.erase_typed_partial();
                    if !self.recording {
                        continue;
                    }
                    if self.auto_insert && !text.is_empty() {
                        let final_text = if text.ends_with('\n') { text.clone() } else { format!("{text} ") };
                        self.type_live(&final_text);
                    }
                    if !text.is_empty() {
                        if !self.text.is_empty() && !self.text.ends_with('\n') {
                            self.text.push(' ');
                        }
                        let start = self.text.len();
                        self.text.push_str(&text);
                        self.segments.push(Segment { range: start..self.text.len(), at: self.started.elapsed() });
                    }
                }
            }
        }
    }

    /// Übernimmt eine neue Serveradresse; das Backend läuft nur während einer Aufnahme.
    fn restart(&mut self, server: String) {
        self.server = server;
        if self.recording {
            self.stop_backend();
            self.start_backend();
        }
    }

    fn start_backend(&mut self) {
        self.stop = Arc::new(AtomicBool::new(false));
        self.status = format!("Verbinde mit {} ...", self.server);
        let tap = self.begin_clip();
        self.backend = Some(spawn_backend(&self.server, self.model, &self.prompt, self.mic.clone(), tap, self.stop.clone(), make_send(self.tx.clone())));
    }

    /// Legt eine neue Aufnahme an und gibt den Sender zurück, über den das Mikrofon in die WAV-Datei schreibt.
    fn begin_clip(&mut self) -> Option<Sender<Vec<f32>>> {
        let dir = audio_dir()?;
        if let Err(err) = std::fs::create_dir_all(&dir) {
            self.status = format!("Aufnahme nicht gespeichert: {err}");
            return None;
        }
        let sample_rate = audio::input_sample_rate(self.mic.as_deref()).ok()?;
        let id = self.next_clip_id;
        self.next_clip_id += 1;
        let path = dir.join(format!("clip-{id}.wav"));
        match recording::start_writer(path.clone(), sample_rate) {
            Ok(tap) => {
                self.clips.push(Clip {
                    id,
                    clock: chrono::Local::now().format("%H:%M:%S").to_string(),
                    start: self.started.elapsed(),
                    end: None,
                    path,
                });
                Some(tap)
            }
            Err(err) => {
                self.status = format!("Aufnahme nicht gespeichert: {err}");
                None
            }
        }
    }

    /// Beendet Mikrofon und Verbindung, sodass kein Audio mehr gesendet wird.
    fn stop_backend(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        join_backend(self.backend.take());
        let now = self.started.elapsed();
        if let Some(clip) = self.clips.iter_mut().rev().find(|clip| clip.end.is_none()) {
            clip.end = Some(now);
        }
    }

    fn window_target(&self) -> Size {
        let base = if self.show_settings { Size::new(700.0, 780.0) } else { self.window_size };
        if self.panel_open { Size::new(base.width + PANEL_WIDTH, base.height) } else { base }
    }

    fn open_audio_folder(&mut self) {
        let Some(dir) = audio_dir() else { return };
        let opened = std::fs::create_dir_all(&dir).and_then(|_| std::process::Command::new("explorer").arg(&dir).spawn().map(|_| ()));
        if let Err(err) = opened {
            self.status = format!("Ordner konnte nicht geöffnet werden: {err}");
        }
    }

    fn select_clip(&mut self, id: u64) {
        let deselect = self.selected_clip == Some(id);
        self.player = None;
        self.selected_clip = None;
        if deselect {
            return;
        }
        let Some(clip) = self.clips.iter().find(|clip| clip.id == id) else { return };
        if clip.end.is_none() {
            self.status = "Aufnahme läuft noch".to_string();
            return;
        }
        match recording::read_wav(&clip.path) {
            Ok((sample_rate, samples)) => match Player::new(sample_rate, samples) {
                Ok(player) => {
                    self.player = Some(player);
                    self.selected_clip = Some(id);
                }
                Err(err) => self.status = format!("Wiedergabe nicht möglich: {err}"),
            },
            Err(err) => self.status = format!("Aufnahme konnte nicht geladen werden: {err}"),
        }
    }

    fn ensure_enigo(&mut self) -> bool {
        if self.enigo.is_some() {
            return true;
        }
        match Enigo::new(&Settings::default()) {
            Ok(enigo) => {
                self.enigo = Some(enigo);
                true
            }
            Err(err) => {
                self.status = format!("Automatisch Einfügen nicht verfügbar: {err}");
                false
            }
        }
    }

    /// Ersetzt die getippte Vorschau im Zielfenster, ohne bereits passende Zeichen neu zu tippen.
    fn update_typed_partial(&mut self, text: &str) {
        if text == self.typed_partial {
            return;
        }
        let old: Vec<char> = self.typed_partial.chars().collect();
        let new: Vec<char> = text.chars().collect();
        let common = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        let rest: String = new[common..].iter().collect();
        self.press_backspace(old.len() - common);
        self.type_live(&rest);
        self.typed_partial = text.to_string();
    }

    fn erase_typed_partial(&mut self) {
        let count = self.typed_partial.chars().count();
        self.typed_partial.clear();
        self.press_backspace(count);
    }

    fn press_backspace(&mut self, count: usize) {
        if count == 0 || !self.ensure_enigo() {
            return;
        }
        if let Some(enigo) = self.enigo.as_mut() {
            for _ in 0..count {
                if let Err(err) = enigo.key(enigo::Key::Backspace, enigo::Direction::Click) {
                    self.status = format!("Automatisch Einfügen fehlgeschlagen: {err}");
                    break;
                }
            }
        }
    }

    /// Types text into the window that currently has keyboard focus.
    fn type_live(&mut self, text: &str) {
        if !self.ensure_enigo() {
            return;
        }
        if let Some(enigo) = self.enigo.as_mut() {
            for (i, line) in text.split('\n').enumerate() {
                if i > 0 {
                    if let Err(err) = enigo.key(enigo::Key::Return, enigo::Direction::Click) {
                        self.status = format!("Automatisch Einfügen fehlgeschlagen: {err}");
                    }
                }
                if !line.is_empty() {
                    if let Err(err) = enigo.text(line) {
                        self.status = format!("Automatisch Einfügen fehlgeschlagen: {err}");
                    }
                }
            }
        }
    }
}
