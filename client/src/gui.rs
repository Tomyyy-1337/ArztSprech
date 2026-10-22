use std::sync::mpsc::{self, Receiver};

use eframe::egui;

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
        viewport: egui::ViewportBuilder::default().with_inner_size([700.0, 450.0]),
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
            spawn_backend(send);
            Ok(Box::new(App { rx, text: String::new(), partial: String::new(), status: String::new() }))
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

struct App {
    rx: Receiver<Msg>,
    text: String,
    partial: String,
    status: String,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Status(status) => self.status = status,
                Msg::Partial(text) => self.partial = text,
                Msg::Final(text) => {
                    self.partial.clear();
                    if !text.is_empty() {
                        if !self.text.is_empty() {
                            self.text.push(' ');
                        }
                        self.text.push_str(&text);
                    }
                }
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                if ui.button("Kopieren").clicked() {
                    ui.ctx().copy_text(self.text.clone());
                }
                if ui.button("Löschen").clicked() {
                    self.text.clear();
                    self.partial.clear();
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.text).desired_width(f32::INFINITY).frame(egui::Frame::NONE));
                ui.weak(&self.partial);
            });
        });
    }
}
