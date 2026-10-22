use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use enigo::{Enigo, Keyboard, Settings};

use arztsprech_core::protocol::{Model, Msg};
#[cfg(feature = "remote")]
use crate::client;
#[cfg(not(feature = "remote"))]
use arztsprech_core::audio;
#[cfg(not(feature = "remote"))]
use arztsprech_core::transcription::{Transcriber, run_transcriber};
#[cfg(not(feature = "remote"))]
use std::error::Error;

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([500.0, 130.0])
            .with_always_on_top(),
        ..Default::default()
    };

    eframe::run_native(
        "Arztsprech",
        options,
        Box::new(|cc| {
            let (tx, rx) = mpsc::channel();
            let server = default_server();
            let model = default_model();
            let prompt = load_prompt();
            let stop = Arc::new(AtomicBool::new(false));
            let backend = spawn_backend(&server, model, &prompt, stop.clone(), make_send(tx.clone(), cc.egui_ctx.clone()));
            apply_style(&cc.egui_ctx);
            Ok(Box::new(App {
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
                prompt: prompt.clone(),
                prompt_input: prompt,
                stop,
                show_settings: false,
                backend: Some(backend),
            }))
        }),
    )
}

#[cfg(feature = "remote")]
const DEFAULT_SERVER: &str = "127.0.0.1:7878";
const DEFAULT_PROMPT: &str = "Deutschsprachiges gastroenterologisches Diktat. Fachbegriffe: ";

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
        .unwrap_or(Model::Turbo)
}

fn save_model(model: Model) {
    write_config("model.txt", model_key(model));
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
        Model::LargeV3 => "large-v3",
    }
}

fn model_from_key(key: &str) -> Option<Model> {
    Model::ALL.into_iter().find(|&model| model_key(model) == key)
}

fn model_label(model: Model) -> &'static str {
    match model {
        Model::Turbo => "large-v3-turbo (schnell)",
        Model::LargeV3 => "large-v3 (genauer)",
    }
}

#[cfg(not(feature = "remote"))]
fn default_server() -> String {
    String::new()
}

fn make_send(tx: Sender<Msg>, ctx: egui::Context) -> impl Fn(Msg) + Clone + Send + 'static {
    move |msg: Msg| {
        let _ = tx.send(msg);
        ctx.request_repaint();
    }
}

#[cfg(feature = "remote")]
fn spawn_backend(server: &str, model: Model, prompt: &str, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) -> std::thread::JoinHandle<()> {
    let server = server.to_string();
    let prompt = prompt.to_string();
    std::thread::spawn(move || {
        let error_send = send.clone();
        if let Err(err) = client::stream_microphone(&server, model, &prompt, stop, send) {
            error_send(Msg::Status(format!("Fehler: {err}")));
        }
    })
}

#[cfg(not(feature = "remote"))]
fn spawn_backend(_server: &str, model: Model, prompt: &str, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) -> std::thread::JoinHandle<()> {
    let prompt = prompt.to_string();
    std::thread::spawn(move || {
        let error_send = send.clone();
        if let Err(err) = run_standalone(model, &prompt, stop, send) {
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
fn run_standalone(model: Model, prompt: &str, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Send + 'static) -> Result<(), Box<dyn Error>> {
    let gate = stop.clone();
    let send = move |msg: Msg| {
        if !gate.load(Ordering::SeqCst) {
            send(msg);
        }
    };
    send(Msg::Status(format!("Lade Modell {} ...", model_label(model))));
    let transcriber = cached_transcriber(model)?;
    let prompt = (!prompt.trim().is_empty()).then_some(prompt);
    run_transcriber(transcriber, audio::open_microphone()?, &stop, prompt, send)
}

fn apply_style(ctx: &egui::Context) {
    const ACCENT: egui::Color32 = egui::Color32::from_rgb(88, 130, 255);
    const SURFACE: egui::Color32 = egui::Color32::from_rgb(30, 32, 38);
    const SURFACE_HOVER: egui::Color32 = egui::Color32::from_rgb(44, 47, 56);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(22, 23, 28);
    visuals.window_fill = visuals.panel_fill;
    visuals.window_corner_radius = egui::CornerRadius::same(10);
    visuals.selection.bg_fill = ACCENT;
    visuals.hyperlink_color = ACCENT;

    let radius = egui::CornerRadius::same(6);
    for widget in [&mut visuals.widgets.inactive, &mut visuals.widgets.noninteractive] {
        widget.weak_bg_fill = SURFACE;
        widget.bg_fill = SURFACE;
        widget.bg_stroke = egui::Stroke::NONE;
        widget.corner_radius = radius;
    }
    visuals.widgets.hovered.weak_bg_fill = SURFACE_HOVER;
    visuals.widgets.hovered.bg_fill = SURFACE_HOVER;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.weak_bg_fill = ACCENT;
    visuals.widgets.active.bg_fill = ACCENT;
    visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
    visuals.widgets.active.corner_radius = radius;

    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
    });
}

struct App {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    text: String,
    partial: String,
    status: String,
    live: bool,
    enigo: Option<Enigo>,
    server: String,
    server_input: String,
    model: Model,
    prompt: String,
    prompt_input: String,
    stop: Arc<AtomicBool>,
    show_settings: bool,
    backend: Option<std::thread::JoinHandle<()>>,
}

impl App {
    /// Beendet die aktuelle Verbindung und verbindet neu mit `server_input`.
    fn reconnect(&mut self, ctx: &egui::Context) {
        let server = self.server_input.trim().to_string();
        if server.is_empty() {
            return;
        }
        save_server(&server);
        self.restart(ctx, server);
    }

    /// Beendet die aktuelle Verbindung und startet das Backend mit dem aktuellen Modell neu.
    fn restart(&mut self, ctx: &egui::Context, server: String) {
        self.stop.store(true, Ordering::SeqCst);
        join_backend(self.backend.take());
        self.stop = Arc::new(AtomicBool::new(false));
        self.server = server;
        self.status = format!("Verbinde mit {} ...", self.server);
        self.backend = Some(spawn_backend(&self.server, self.model, &self.prompt, self.stop.clone(), make_send(self.tx.clone(), ctx.clone())));
    }

    /// Types text into the window that currently has keyboard focus.
    fn type_live(&mut self, text: &str) {
        if self.enigo.is_none() {
            match Enigo::new(&Settings::default()) {
                Ok(enigo) => self.enigo = Some(enigo),
                Err(err) => {
                    self.status = format!("Live-Eingabe nicht verfügbar: {err}");
                    self.live = false;
                    return;
                }
            }
        }
        if let Some(enigo) = self.enigo.as_mut() {
            if let Err(err) = enigo.text(text) {
                self.status = format!("Live-Eingabe fehlgeschlagen: {err}");
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut text_changed = false;
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Status(status) => self.status = status,
                Msg::Partial(text) => self.partial = text,
                Msg::Final(text) => {
                    self.partial.clear();
                    if !text.is_empty() {
                        if self.live {
                            self.type_live(&format!("{text} "));
                        }
                        if !self.text.is_empty() {
                            self.text.push(' ');
                        }
                        self.text.push_str(&text);
                        text_changed = true;
                    }
                }
            }
        }

        egui::Panel::top("status").show(ui, |ui| {
            let frame = egui::Frame::NONE
                .fill(egui::Color32::from_rgb(30, 32, 38))
                .inner_margin(egui::Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.label(
                        egui::RichText::new(format!("Status: {}", self.status))
                            .color(egui::Color32::from_rgb(120, 160, 255)),
                    );
                });
            if self.live {
                let rect = frame.response.rect;
                ui.painter().circle_filled(egui::pos2(rect.right() - 12.0, rect.center().y), 5.0, egui::Color32::RED);
            }
        });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Kopieren").clicked() {
                    ui.ctx().copy_text(self.text.clone());
                }
                let live_toggle = ui.toggle_value(&mut self.live, "Live-Eingabe");
                if live_toggle.changed() && self.live {
                    self.text.clear();
                    self.partial.clear();
                }
                if ui.button("Löschen").clicked() {
                    self.text.clear();
                    self.partial.clear();
                }
                if ui.button("Einstellungen").clicked() {
                    self.show_settings = !self.show_settings;
                }
            });
            ui.separator();
            if self.show_settings {
                let remote = cfg!(feature = "remote");
                let mut apply = false;
                let mut apply_prompt = false;
                let mut model_changed = false;
                let mut show_settings = self.show_settings;
                egui::Window::new("Einstellungen")
                    .open(&mut show_settings)
                    .resizable(true)
                    .default_width(320.0)
                    .show(ui.ctx(), |ui| {
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    if remote {
                                        ui.label("Server (IP:Port)");
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::TextEdit::singleline(&mut self.server_input).desired_width(180.0),
                                            );
                                            if ui.button("Übernehmen").clicked() {
                                                apply = true;
                                            }
                                        });
                                        ui.weak(format!("Aktuell: {}", self.server));
                                    }
                                    ui.label("Modell");
                                    let before = self.model;
                                    egui::ComboBox::from_id_salt("model")
                                        .selected_text(model_label(self.model))
                                        .show_ui(ui, |ui| {
                                            for model in Model::ALL {
                                                ui.selectable_value(&mut self.model, model, model_label(model));
                                            }
                                        });
                                    model_changed = self.model != before;
                                    ui.separator();
                                    ui.label("Systemprompt");
                                    ui.add(
                                        egui::TextEdit::multiline(&mut self.prompt_input)
                                            .desired_width(300.0)
                                            .desired_rows(5),
                                    );
                                    if ui.button("Systemprompt übernehmen").clicked() {
                                        apply_prompt = true;
                                    }
                                });
                    });
                self.show_settings = show_settings;
                if apply {
                    let ctx = ui.ctx().clone();
                    self.reconnect(&ctx);
                }
                if model_changed {
                    save_model(self.model);
                    let ctx = ui.ctx().clone();
                    let server = self.server.clone();
                    self.restart(&ctx, server);
                }
                if apply_prompt {
                    self.prompt = self.prompt_input.trim().to_string();
                    save_prompt(&self.prompt);
                    let ctx = ui.ctx().clone();
                    let server = self.server.clone();
                    self.restart(&ctx, server);
                }
            }
            let font = egui::TextStyle::Body.resolve(ui.style());
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &self.text,
                0.0,
                egui::TextFormat {
                    font_id: font.clone(),
                    color: ui.visuals().text_color(),
                    ..Default::default()
                },
            );
            if !self.partial.is_empty() {
                let sep = if self.text.is_empty() { "" } else { " " };
                job.append(
                    &format!("{sep}{}", self.partial),
                    0.0,
                    egui::TextFormat {
                        font_id: font,
                        color: ui.visuals().weak_text_color(),
                        ..Default::default()
                    },
                );
            }
            let out = egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(job).wrap());
                    if text_changed {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });
        });
    }
}
