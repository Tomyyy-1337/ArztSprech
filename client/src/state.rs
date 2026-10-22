use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use enigo::{Enigo, Keyboard, Settings};
use iced::widget::text_editor;
use iced::{Size, Subscription, Task, Theme, window};

use arztsprech_core::audio;
use arztsprech_core::protocol::{Model, Msg};
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

#[derive(Debug, Clone)]
pub enum Message {
    Tick,
    Copy,
    Clear,
    ToggleLive(bool),
    ToggleSettings,
    WindowResized(Size),
    ServerInput(String),
    ApplyServer,
    ModelSelected(Model),
    MicSelected(String),
    PromptEdit(text_editor::Action),
    ApplyPrompt,
}

pub struct App {
    pub(super) rx: Receiver<Msg>,
    pub(super) tx: Sender<Msg>,
    pub(super) text: String,
    pub(super) partial: String,
    pub(super) status: String,
    pub(super) live: bool,
    pub(super) enigo: Option<Enigo>,
    pub(super) server: String,
    pub(super) server_input: String,
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
}

pub fn boot() -> App {
    let (tx, rx) = mpsc::channel();
    let server = default_server();
    let model = default_model();
    let prompt = load_prompt();
    let mic = load_mic();
    let stop = Arc::new(AtomicBool::new(false));
    let backend = spawn_backend(&server, model, &prompt, mic.clone(), stop.clone(), make_send(tx.clone()));
    App {
        rx,
        tx,
        text: String::new(),
        partial: String::new(),
        status: String::new(),
        live: false,
        enigo: None,
        server: server.clone(),
        server_input: server,
        model,
        prompt_input: text_editor::Content::with_text(&prompt),
        prompt,
        mic,
        mic_options: audio::input_device_names(),
        stop,
        show_settings: false,
        window_size: Size::new(500.0, 180.0),
        backend: Some(backend),
        theme: app_theme(),
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
        Model::Turbo => "large-v3-turbo (schnell)",
        Model::GermanTurbo => "german-turbo (deutsche Fachbegriffe)",
    }
}

fn make_send(tx: Sender<Msg>) -> impl Fn(Msg) + Clone + Send + 'static {
    move |msg: Msg| {
        let _ = tx.send(msg);
    }
}

#[cfg(feature = "remote")]
fn spawn_backend(server: &str, model: Model, prompt: &str, mic: Option<String>, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) -> std::thread::JoinHandle<()> {
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
        if let Err(err) = client::stream_microphone(&server, model, &prompt, mic, stop, send) {
            error_send(Msg::Status(format!("Fehler: {err}")));
        }
    })
}

#[cfg(not(feature = "remote"))]
fn spawn_backend(_server: &str, model: Model, prompt: &str, mic: Option<String>, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) -> std::thread::JoinHandle<()> {
    let prompt = prompt.to_string();
    std::thread::spawn(move || {
        let error_send = send.clone();
        if let Err(err) = run_standalone(model, &prompt, mic, stop, send) {
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

/// Loads each model once and keeps it; reloading on every switch exhausted the GPU.
#[cfg(not(feature = "remote"))]
fn cached_transcriber(model: Model) -> Result<&'static Transcriber, Box<dyn Error>> {
    static CACHE: [std::sync::OnceLock<Transcriber>; 2] = [std::sync::OnceLock::new(), std::sync::OnceLock::new()];
    let slot = &CACHE[model as usize];
    if let Some(transcriber) = slot.get() {
        return Ok(transcriber);
    }
    let transcriber = Transcriber::load(model.path())?;
    Ok(slot.get_or_init(|| transcriber))
}

#[cfg(not(feature = "remote"))]
fn run_standalone(model: Model, prompt: &str, mic: Option<String>, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Send + 'static) -> Result<(), Box<dyn Error>> {
    let gate = stop.clone();
    let send = move |msg: Msg| {
        if !gate.load(Ordering::SeqCst) {
            send(msg);
        }
    };
    send(Msg::Status(format!("Lade Modell {} ...", model_label(model))));
    let transcriber = cached_transcriber(model)?;
    let prompt = (!prompt.trim().is_empty()).then_some(prompt);
    run_transcriber(transcriber, audio::open_microphone(mic.as_deref())?, &stop, prompt, send)
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
                self.partial.clear();
            }
            Message::ToggleLive(live) => {
                self.live = live;
                if live {
                    self.text.clear();
                    self.partial.clear();
                }
            }
            Message::ToggleSettings => {
                self.show_settings = !self.show_settings;
                if self.show_settings {
                    self.mic_options = audio::input_device_names();
                }
                let size = if self.show_settings { Size::new(700.0, 600.0) } else { self.window_size };
                return window::latest().and_then(move |id| window::resize(id, size));
            }
            Message::WindowResized(size) => {
                if !self.show_settings {
                    self.window_size = size;
                }
            }
            Message::ServerInput(server) => self.server_input = server,
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
                Msg::Partial(text) => self.partial = text,
                Msg::Final(text) => {
                    self.partial.clear();
                    if self.live && !text.is_empty() {
                        let final_text = if text.ends_with('\n') { text.clone() } else { format!("{text} ") };
                        self.type_live(&final_text);
                    }
                    if !text.is_empty() {
                        if !self.text.is_empty() && !self.text.ends_with('\n') {
                            self.text.push(' ');
                        }
                        self.text.push_str(&text);
                    }
                }
            }
        }
    }

    /// Beendet die aktuelle Verbindung und startet das Backend mit dem aktuellen Modell neu.
    fn restart(&mut self, server: String) {
        self.stop.store(true, Ordering::SeqCst);
        join_backend(self.backend.take());
        self.stop = Arc::new(AtomicBool::new(false));
        self.server = server;
        self.status = format!("Verbinde mit {} ...", self.server);
        self.backend = Some(spawn_backend(&self.server, self.model, &self.prompt, self.mic.clone(), self.stop.clone(), make_send(self.tx.clone())));
    }

    /// Types text into the window that currently has keyboard focus.
    fn type_live(&mut self, text: &str) {
        if self.enigo.is_none() {
            match Enigo::new(&Settings::default()) {
                Ok(enigo) => self.enigo = Some(enigo),
                Err(err) => {
                    self.status = format!("Automatisch Einfügen nicht verfügbar: {err}");
                    self.live = false;
                    return;
                }
            }
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
