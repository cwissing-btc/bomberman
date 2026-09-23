//! Bomberman player client.
//!
//! One window for everything a player needs: join a server, see who is in the
//! lobby, start the match, play it by hand or let the built-in bot play, and
//! see the results. Talks to the arena over its UDP bot protocol only.

// No console window behind the game on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;
mod assets;
mod board;
use bomber_bot::bot;
mod fx;
mod input;
mod net;
mod screens;
mod session;
mod settings;
mod sound;
mod ui;
use bomber_bot::world;

use macroquad::prelude::*;

use app::{App, Options};
use settings::Settings;

fn window_conf() -> Conf {
    Conf {
        window_title: "Bomberman".to_owned(),
        window_width: 1280,
        window_height: 800,
        high_dpi: true,
        window_resizable: true,
        sample_count: 4,
        icon: assets::window_icon(),
        ..Default::default()
    }
}

const USAGE: &str = "\
Bomberman-Client

  bomberman [--host ADRESSE] [--port PORT] [--name NAME] [--bot] [--connect] [--autostart]

  --host       Serveradresse (Vorgabe: zuletzt benutzt, sonst 127.0.0.1)
  --port       UDP-Port des Servers (Vorgabe 47800)
  --name       Anzeigename, höchstens 24 Byte
  --bot        der eingebaute Bot spielt
  --connect    Menü überspringen und sofort verbinden
  --autostart  Match automatisch starten, sobald es geht (für Bot-Runden)
";

fn parse_args() -> Options {
    let mut settings = Settings::load();
    let mut connect = false;
    let mut autostart = false;
    let mut debug = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut value = || inline.clone().or_else(|| args.next()).unwrap_or_default();
        match key.as_str() {
            "--host" => settings.set_address(&value()),
            "--port" => settings.port = value().parse().unwrap_or(settings.port),
            "--name" => settings.name = value().chars().take(24).collect(),
            "--bot" => settings.bot = true,
            "--connect" => connect = true,
            "--autostart" => autostart = true,
            "--debug" => debug = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => {}
        }
    }
    Options {
        settings,
        connect,
        autostart,
        debug,
    }
}

/// Without a console, a panic would vanish. Write it next to the settings.
fn install_crash_log() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = if cfg!(windows) {
            std::env::var_os("APPDATA").map(std::path::PathBuf::from)
        } else {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        };
        if let Some(dir) = dir.map(|d| d.join("Bomberman-L4D")) {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("crash.log"), format!("{info}\n"));
        }
        default(info);
    }));
}

#[macroquad::main(window_conf)]
async fn main() {
    install_crash_log();
    let options = parse_args();
    let mut app = App::new(options).await;
    loop {
        app.frame();
        next_frame().await;
    }
}
