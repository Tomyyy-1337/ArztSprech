#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(feature = "remote")]
mod client;
mod layout;
mod state;

use state::{App, boot};
use iced::window;

fn main() -> iced::Result {
    iced::application(boot, App::update, App::view)
        .title("Arztsprech")
        .theme(App::theme)
        .subscription(App::subscription)
        .window_size((500.0, 180.0))
        .level(window::Level::AlwaysOnTop)
        .run()
}

