#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() -> eframe::Result {
    arztsprech_client::gui::run()
}

