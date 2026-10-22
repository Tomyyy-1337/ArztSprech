use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use enigo::{Enigo, Keyboard, Settings};
use iced::theme::Palette;
use iced::widget::{button, column, container, pick_list, row, rich_text, scrollable, space, span, text, text_editor, text_input, toggler};
use iced::{Background, Border, Color, Element, Fill, Length, Size, Subscription, Task, Theme, window};

use arztsprech_core::audio;
use arztsprech_core::protocol::{Model, Msg};
#[cfg(feature = "remote")]
use crate::client;
#[cfg(not(feature = "remote"))]
use arztsprech_core::transcription::{Transcriber, run_transcriber};
#[cfg(not(feature = "remote"))]
use std::error::Error;

pub fn run() -> iced::Result {
    iced::application(boot, App::update, App::view)
        .title("Arztsprech")
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size((500.0, 180.0))
        .level(window::Level::AlwaysOnTop)
        .run()
}

fn boot() -> App {
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
        live_partial: String::new(),
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
const DEFAULT_SERVER: &str = "127.0.0.1:7878";
const DEFAULT_PROMPT: &str = "Deutschsprachiges gastroenterologisches Diktat. Fachbegriffe: ";
const SYSTEM_MIC: &str = "Systemstandard";

#[cfg(feature = "remote")]
fn default_server() -> String {
    load_server()
        .or_else(|| std::env::var("ARZTSPRECH_SERVER").ok())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
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

fn model_label(model: Model) -> &'static str {
    match model {
        Model::Turbo => "large-v3-turbo (schnell)",
        Model::GermanTurbo => "german-turbo (deutsche Fachbegriffe)",
    }
}

#[cfg(not(feature = "remote"))]
fn default_server() -> String {
    String::new()
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

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

fn app_theme() -> Theme {
    Theme::custom(
        "Arztsprech".to_string(),
        Palette {
            background: rgb(22, 23, 28),
            text: rgb(220, 222, 228),
            primary: rgb(88, 130, 255),
            success: rgb(90, 200, 120),
            warning: rgb(230, 180, 60),
            danger: rgb(235, 70, 70),
        },
    )
}

fn surface_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(rgb(30, 32, 38))),
        ..Default::default()
    }
}

fn outline_style(_: &Theme) -> container::Style {
    container::Style {
        border: Border { color: rgb(88, 92, 104), width: 1.0, radius: 6.0.into() },
        ..Default::default()
    }
}

fn live_dot_style(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color::from_rgb(1.0, 0.0, 0.0))),
        border: Border { radius: 5.0.into(), ..Default::default() },
        ..Default::default()
    }
}

/// Wrapper so `pick_list` can display a `Model` via `model_label`.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ModelChoice(Model);

impl std::fmt::Display for ModelChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", model_label(self.0))
    }
}

#[derive(Debug, Clone)]
enum Message {
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

struct App {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    text: String,
    partial: String,
    status: String,
    live: bool,
    live_partial: String,
    enigo: Option<Enigo>,
    server: String,
    server_input: String,
    model: Model,
    prompt: String,
    prompt_input: text_editor::Content,
    mic: Option<String>,
    mic_options: Vec<String>,
    stop: Arc<AtomicBool>,
    show_settings: bool,
    window_size: Size,
    backend: Option<std::thread::JoinHandle<()>>,
    theme: Theme,
}

impl App {
    fn theme(&self) -> Theme {
        self.theme.clone()
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            window::frames().map(|_| Message::Tick),
            window::resize_events().map(|(_, size)| Message::WindowResized(size)),
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => self.drain_backend(),
            Message::Copy => return iced::clipboard::write(self.text.clone()),
            Message::Clear => {
                if self.live {
                    self.replace_live_partial("");
                }
                self.text.clear();
                self.partial.clear();
                self.live_partial.clear();
            }
            Message::ToggleLive(live) => {
                self.live = live;
                if live {
                    self.text.clear();
                    self.partial.clear();
                    self.live_partial.clear();
                } else {
                    self.replace_live_partial("");
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
                Msg::Partial(text) => {
                    self.partial = text;
                    if self.live {
                        let partial = self.partial.clone();
                        self.replace_live_partial(&partial);
                    }
                }
                Msg::Final(text) => {
                    self.partial.clear();
                    if self.live {
                        let final_text = if text.is_empty() { String::new() } else { format!("{text} ") };
                        self.replace_live_partial(&final_text);
                        self.live_partial.clear();
                    }
                    if !text.is_empty() {
                        if !self.text.is_empty() {
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
            if let Err(err) = enigo.text(text) {
                self.status = format!("Automatisch Einfügen fehlgeschlagen: {err}");
            }
        }
    }

    /// Erases back to the start of the first changed word and retypes the rest.
    fn replace_live_partial(&mut self, text: &str) {
        let old: Vec<char> = self.live_partial.chars().collect();
        let new: Vec<char> = text.chars().collect();
        let mut common = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
        if common < old.len() {
            while common > 0 && !old[common - 1].is_whitespace() {
                common -= 1;
            }
        }
        for _ in common..old.len() {
            self.send_live_key(enigo::Key::Backspace, enigo::Direction::Click);
        }
        let new_tail: String = new[common..].iter().collect();
        if !new_tail.is_empty() {
            self.type_live(&new_tail);
        }
        self.live_partial = text.to_string();
    }

    fn send_live_key(&mut self, key: enigo::Key, direction: enigo::Direction) {
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
            if let Err(err) = enigo.key(key, direction) {
                self.status = format!("Automatisch Einfügen fehlgeschlagen: {err}");
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        if self.show_settings {
            return self.settings_view();
        }
        let status_color = rgb(120, 160, 255);
        let mut status_row = row![text(format!("Status: {}", self.status)).size(12).color(status_color).width(Fill)]
            .spacing(8)
            .align_y(iced::alignment::Vertical::Center);
        if self.live {
            status_row = status_row.push(container("").width(10).height(10).style(live_dot_style));
        }
        let status_bar = container(status_row).padding([6, 10]).width(Fill).style(surface_style);

        let toolbar = row![
            button("Kopieren").on_press(Message::Copy),
            button("Löschen").on_press(Message::Clear),
            button("Einstellungen").on_press(Message::ToggleSettings),
            space::horizontal(),
            text("Automatisch\nEinfügen"),
            toggler(self.live).on_toggle(Message::ToggleLive),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center);

        let body = column![status_bar, toolbar, self.transcript_view()].spacing(8).padding(8);
        container(body).width(Fill).height(Fill).into()
    }

    fn settings_view(&self) -> Element<'_, Message> {
        let header = row![
            text("Einstellungen").size(20).width(Fill),
            button("X").on_press(Message::ToggleSettings),
        ]
        .align_y(iced::alignment::Vertical::Center);

        let mut content = column![].spacing(8).width(Fill);
        if cfg!(feature = "remote") {
            content = content
                .push(text("Server (IP:Port)"))
                .push(
                    row![
                        text_input("127.0.0.1:7878", &self.server_input)
                            .on_input(Message::ServerInput)
                            .on_submit(Message::ApplyServer)
                            .width(Fill),
                        button("Übernehmen").on_press(Message::ApplyServer),
                    ]
                    .spacing(8),
                )
                .push(text(format!("Aktuell: {}", self.server)).color(rgb(140, 144, 155)));
        }
        let models: Vec<ModelChoice> = Model::ALL.map(ModelChoice).to_vec();
        let mut mic_options = vec![SYSTEM_MIC.to_string()];
        mic_options.extend(self.mic_options.iter().cloned());
        content = content
            .push(text("Modell"))
            .push(
                pick_list(models, Some(ModelChoice(self.model)), |choice: ModelChoice| {
                    Message::ModelSelected(choice.0)
                })
                .width(Fill),
            )
            .push(text("Mikrofon"))
            .push(
                pick_list(mic_options, Some(self.mic.clone().unwrap_or_else(|| SYSTEM_MIC.to_string())), Message::MicSelected)
                    .width(Fill),
            )
            .push(text("Systemprompt"))
            .push(container(text_editor(&self.prompt_input).on_action(Message::PromptEdit).height(200)).style(outline_style))
            .push(button("Systemprompt übernehmen").on_press(Message::ApplyPrompt));

        let page = column![header, scrollable(content).width(Fill).height(Fill)].spacing(12);
        container(page)
            .padding(12)
            .width(Fill)
            .height(Fill)
            .style(surface_style)
            .into()
    }

    fn transcript_view(&self) -> Element<'_, Message> {
        let mut spans = vec![span::<(), _>(self.text.clone())];
        if !self.partial.is_empty() {
            let sep = if self.text.is_empty() { "" } else { " " };
            spans.push(span::<(), _>(format!("{sep}{}", self.partial)).color(rgb(120, 124, 135)));
        }
        container(
            scrollable(rich_text(spans).width(Fill))
                .anchor_bottom()
                .width(Fill)
                .height(Length::Fill),
        )
        .padding(8)
        .width(Fill)
        .height(Length::Fill)
        .style(outline_style)
        .into()
    }
}
