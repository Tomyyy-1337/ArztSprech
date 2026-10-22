use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;
use enigo::{Enigo, Keyboard, Settings};

use arztsprech_core::protocol::Msg;
#[cfg(feature = "remote")]
use crate::client;
#[cfg(not(feature = "remote"))]
use arztsprech_core::audio;
#[cfg(not(feature = "remote"))]
use arztsprech_core::transcription::{DEFAULT_MODEL, Transcriber, run_transcriber};
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
            let stop = Arc::new(AtomicBool::new(false));
            spawn_backend(&server, stop.clone(), make_send(tx.clone(), cc.egui_ctx.clone()));
            apply_style(&cc.egui_ctx);
            Ok(Box::new(App {
                rx,
                tx,
                text: String::new(),
                partial: String::new(),
                status: String::new(),
                live: false,
                enigo: None,
                overflow: false,
                server: server.clone(),
                server_input: server,
                stop,
                show_settings: false,
            }))
        }),
    )
}

#[cfg(feature = "remote")]
const DEFAULT_SERVER: &str = "127.0.0.1:7878";

#[cfg(feature = "remote")]
fn default_server() -> String {
    load_server()
        .or_else(|| std::env::var("ARZTSPRECH_SERVER").ok())
        .unwrap_or_else(|| DEFAULT_SERVER.to_string())
}

/// Speicherort: `%APPDATA%\arztsprech\server.txt`.
fn config_path() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|dir| std::path::PathBuf::from(dir).join("arztsprech").join("server.txt"))
}

#[cfg(feature = "remote")]
fn load_server() -> Option<String> {
    let server = std::fs::read_to_string(config_path()?).ok()?;
    let server = server.trim().to_string();
    (!server.is_empty()).then_some(server)
}

fn save_server(server: &str) {
    if let Some(path) = config_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, server);
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
fn spawn_backend(server: &str, stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) {
    let server = server.to_string();
    std::thread::spawn(move || {
        let error_send = send.clone();
        if let Err(err) = client::stream_microphone(&server, stop, send) {
            error_send(Msg::Status(format!("Fehler: {err}")));
        }
    });
}

#[cfg(not(feature = "remote"))]
fn spawn_backend(_server: &str, _stop: Arc<AtomicBool>, send: impl Fn(Msg) + Clone + Send + 'static) {
    std::thread::spawn(move || {
        let error_send = send.clone();
        if let Err(err) = run_standalone(send) {
            error_send(Msg::Status(format!("Fehler: {err}")));
        }
    });
}

#[cfg(not(feature = "remote"))]
fn run_standalone(mut send: impl FnMut(Msg)) -> Result<(), Box<dyn Error>> {
    let model = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_MODEL.to_string());
    send(Msg::Status(format!("Lade Modell {model} ...")));
    let transcriber = Transcriber::load(&model)?;
    run_transcriber(&transcriber, audio::open_microphone()?, send)
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
    overflow: bool,
    server: String,
    server_input: String,
    stop: Arc<AtomicBool>,
    show_settings: bool,
}

impl App {
    /// Beendet die aktuelle Verbindung und verbindet neu mit `server_input`.
    fn reconnect(&mut self, ctx: &egui::Context) {
        let server = self.server_input.trim().to_string();
        if server.is_empty() {
            return;
        }
        self.stop.store(true, Ordering::SeqCst);
        self.stop = Arc::new(AtomicBool::new(false));
        self.server = server;
        save_server(&self.server);
        self.status = format!("Verbinde mit {} ...", self.server);
        spawn_backend(&self.server, self.stop.clone(), make_send(self.tx.clone(), ctx.clone()));
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
                egui::Window::new("Einstellungen")
                    .open(&mut self.show_settings)
                    .resizable(false)
                    .show(ui.ctx(), |ui| {
                        ui.label("Server (IP:Port)");
                        ui.horizontal(|ui| {
                            ui.add_enabled(
                                remote,
                                egui::TextEdit::singleline(&mut self.server_input).desired_width(180.0),
                            );
                            if ui.add_enabled(remote, egui::Button::new("Übernehmen")).clicked() {
                                apply = true;
                            }
                        });
                        if remote {
                            ui.weak(format!("Aktuell: {}", self.server));
                        } else {
                            ui.weak("Nur mit Feature 'remote' verfügbar.");
                        }
                    });
                if apply {
                    let ctx = ui.ctx().clone();
                    self.reconnect(&ctx);
                }
            }
            let overflow = self.overflow;
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
                    if overflow {
                        let line = ui.text_style_height(&egui::TextStyle::Body);
                        ui.add_space(line + ui.spacing().item_spacing.y);
                    }
                    if text_changed {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });
            self.overflow = out.content_size.y > out.inner_rect.height();
        });
    }
}
