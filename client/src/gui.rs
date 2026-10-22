use std::sync::mpsc::{self, Receiver};

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
            let ctx = cc.egui_ctx.clone();
            let send = move |msg: Msg| {
                let _ = tx.send(msg);
                ctx.request_repaint();
            };
            apply_style(&cc.egui_ctx);
            spawn_backend(send);
            Ok(Box::new(App {
                rx,
                text: String::new(),
                partial: String::new(),
                status: String::new(),
                live: false,
                enigo: None,
                overflow: false,
            }))
        }),
    )
}

#[cfg(feature = "remote")]
const DEFAULT_SERVER: &str = "127.0.0.1:7878";

#[cfg(feature = "remote")]
fn spawn_backend(send: impl Fn(Msg) + Clone + Send + 'static) {
    let server = std::env::var("ARZTSPRECH_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_string());
    std::thread::spawn(move || {
        let error_send = send.clone();
        if let Err(err) = client::stream_microphone(&server, send) {
            error_send(Msg::Status(format!("Fehler: {err}")));
        }
    });
}

#[cfg(not(feature = "remote"))]
fn spawn_backend(send: impl Fn(Msg) + Clone + Send + 'static) {
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
    text: String,
    partial: String,
    status: String,
    live: bool,
    enigo: Option<Enigo>,
    overflow: bool,
}

impl App {
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
            });
            ui.separator();
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
