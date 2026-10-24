use std::error::Error;
use std::path::Path;

/// Zugelassene Benutzer aus einer Datei mit Zeilen `benutzername:passwort`.
/// Leere Zeilen und Zeilen mit `#` am Anfang werden ignoriert.
pub struct Users {
    entries: Vec<(String, String)>,
}

impl Users {
    pub fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let text = std::fs::read_to_string(path)
            .map_err(|err| format!("Benutzerliste {} nicht lesbar: {err}", path.display()))?;
        let mut entries = Vec::new();
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty() && !line.starts_with('#')) {
            let (username, password) = line
                .split_once(':')
                .ok_or_else(|| format!("Ungültige Zeile in {} (erwartet benutzername:passwort)", path.display()))?;
            entries.push((username.trim().to_string(), password.trim().to_string()));
        }
        Ok(Self { entries })
    }

    pub fn verify(&self, username: &str, password: &str) -> bool {
        let mut authorized = false;
        for (known_user, known_password) in &self.entries {
            authorized |= constant_time_eq(known_user.as_bytes(), username.as_bytes())
                & constant_time_eq(known_password.as_bytes(), password.as_bytes());
        }
        authorized
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}
