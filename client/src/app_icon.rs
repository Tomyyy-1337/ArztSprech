use iced::window::icon;
use std::path::PathBuf;

static ICON: &[u8] = include_bytes!("../assets/icon.ico");

pub fn install_icon_and_shortcut() {
    let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) else { return };
    let dir = appdata.join("arztsprech");
    let icon_path = dir.join("icon.ico");
    if std::fs::read(&icon_path).map_or(true, |existing| existing != ICON) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&icon_path, ICON);
    }

    let lnk_path = appdata.join(r"Microsoft\Windows\Start Menu\Programs\Arztsprech.lnk");
    if lnk_path.exists() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let Some(exe_dir) = exe.parent() else { return };

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = "$s = (New-Object -ComObject WScript.Shell).CreateShortcut($env:ARZT_LNK); \
            $s.TargetPath = $env:ARZT_EXE; $s.WorkingDirectory = $env:ARZT_DIR; \
            $s.IconLocation = $env:ARZT_ICON + ',0'; $s.Save()";
        let _ = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("ARZT_LNK", &lnk_path)
            .env("ARZT_EXE", &exe)
            .env("ARZT_DIR", exe_dir)
            .env("ARZT_ICON", &icon_path)
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
}

pub fn load_icon() -> Option<icon::Icon> {
    let img = image::load_from_memory_with_format(ICON, image::ImageFormat::Ico).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    icon::from_rgba(img.into_raw(), width, height).ok()
}
