#![cfg_attr(windows, windows_subsystem = "windows")]

mod app_icon;
#[cfg(feature = "remote")]
mod client;
mod layout;
mod state;

use app_icon::{install_icon_and_shortcut, load_icon};
use state::{App, boot};
use iced::{Size, window};

fn main() -> iced::Result {
    install_icon_and_shortcut();
    iced::application(boot, App::update, App::view)
        .title("Arztsprech")
        .theme(App::theme)
        .subscription(App::subscription)
        .window(window::Settings {
            size: Size::new(500.0, 350.0),
            min_size: Some(Size::new(420.0, 180.0)),
            level: window::Level::AlwaysOnTop,
            icon: load_icon(),
            ..Default::default()
        })
        .run()
}