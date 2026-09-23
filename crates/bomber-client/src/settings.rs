//! What the player typed last time, so the next start is one keypress.

use std::fs;
use std::path::PathBuf;

use bomber_protocol::DEFAULT_UDP_PORT;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub sound: bool,
    pub bot: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: default_name(),
            host: "127.0.0.1".into(),
            port: DEFAULT_UDP_PORT,
            sound: true,
            bot: false,
        }
    }
}

fn default_name() -> String {
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let name: String = user.chars().filter(|c| !c.is_control()).take(24).collect();
    if name.is_empty() {
        "Spieler".into()
    } else {
        name
    }
}

/// `%APPDATA%\Bomberman-L4D\client.cfg` on Windows, the XDG config dir elsewhere.
fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("Bomberman-L4D").join("client.cfg"))
}

impl Settings {
    pub fn load() -> Settings {
        let mut settings = Settings::default();
        let Some(text) = path().and_then(|p| fs::read_to_string(p).ok()) else {
            return settings;
        };
        settings.merge(&text);
        settings
    }

    fn merge(&mut self, text: &str) {
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "name" if !value.is_empty() => self.name = value.chars().take(24).collect(),
                "host" if !value.is_empty() => self.host = value.into(),
                "port" => {
                    if let Ok(port) = value.parse() {
                        self.port = port;
                    }
                }
                "sound" => self.sound = value != "0",
                "bot" => self.bot = value == "1",
                _ => {}
            }
        }
    }

    pub fn to_text(&self) -> String {
        format!(
            "name={}\nhost={}\nport={}\nsound={}\nbot={}\n",
            self.name, self.host, self.port, self.sound as u8, self.bot as u8
        )
    }

    /// Best effort: a read-only profile must not stop anyone from playing.
    pub fn save(&self) {
        if let Some(path) = path() {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(path, self.to_text());
        }
    }

    /// `host`, `host:port`, or `[v6]:port`.
    pub fn set_address(&mut self, text: &str) {
        let text = text.trim();
        if let Some(rest) = text.strip_prefix('[') {
            if let Some((host, port)) = rest.split_once("]:") {
                self.host = host.into();
                self.port = port.parse().unwrap_or(self.port);
                return;
            }
        }
        match text.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => {
                self.host = host.into();
                self.port = port.parse().unwrap();
            }
            _ => self.host = text.into(),
        }
    }

    pub fn address(&self) -> String {
        if self.port == DEFAULT_UDP_PORT {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_survive_a_round_trip() {
        let settings = Settings {
            name: "Anna".into(),
            host: "10.0.0.5".into(),
            port: 47801,
            sound: false,
            bot: true,
        };
        let mut loaded = Settings::default();
        loaded.merge(&settings.to_text());
        assert_eq!(loaded, settings);
    }

    #[test]
    fn an_address_may_carry_a_port() {
        let mut s = Settings::default();
        s.set_address("arena.local:5000");
        assert_eq!((s.host.as_str(), s.port), ("arena.local", 5000));
        s.set_address("192.168.1.9");
        assert_eq!((s.host.as_str(), s.port), ("192.168.1.9", 5000));
        s.set_address("[::1]:47800");
        assert_eq!((s.host.as_str(), s.port), ("::1", 47800));
    }
}
