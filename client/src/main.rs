#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> iced::Result {
    arztsprech_client::gui::run()
}

