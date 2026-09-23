//! Full screens and overlays: the connect menu, lobby, HUD, results, pause.
//!
//! Each function draws and reports what the player clicked; the caller owns
//! all state changes, so these stay free of networking.

use macroquad::prelude::*;

use bomber_domain::game::EndReason;
use bomber_domain::lobby::LobbyState;
use bomber_domain::shared::Direction;
use bomber_protocol::TICK_RATE;

use crate::assets::{item_sprite, player_sprite, Sprites};
use crate::session::Session;
use crate::ui::{palette::*, rounded_outline, Align, TextField, Ui};

/// What a click on a screen asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Connect,
    Start,
    ToggleBot,
    ToggleSound,
    Disconnect,
    CloseMenu,
    Pause,
    Resume,
    Abort,
    /// Fill a seat with a server bot, or remove it.
    SeatBot {
        seat: u8,
        enabled: bool,
    },
}

/// A slowly scrolling, darkened floor behind menus and the lobby.
pub fn backdrop(ui: &Ui, sprites: &Sprites) {
    clear_background(BACKGROUND);
    let size = ui.u(72.0);
    let offset = (ui.time as f32 * ui.u(12.0)) % (size * 2.0);
    let cols = (screen_width() / size) as i32 + 3;
    let rows = (screen_height() / size) as i32 + 3;
    for y in -2..rows {
        for x in -2..cols {
            let name = if (x + y).rem_euclid(2) == 0 {
                "floor_0"
            } else {
                "floor_1"
            };
            sprites.draw(
                name,
                x as f32 * size + offset,
                y as f32 * size + offset * 0.5,
                size,
                size,
                Color::new(0.30, 0.34, 0.40, 1.0),
            );
        }
    }
    draw_rectangle(
        0.0,
        0.0,
        screen_width(),
        screen_height(),
        with_alpha(BACKGROUND, 0.72),
    );
}

/// The game's name, bouncing gently, with a lit bomb beside it.
pub fn title(ui: &Ui, sprites: &Sprites, y: f32) {
    let size = ui.u(76.0).min(screen_width() / 9.0);
    let bounce = (ui.time as f32 * 2.2).sin().abs() * ui.u(4.0);
    let cx = screen_width() / 2.0;
    ui.text(
        "BOMBERMAN",
        cx + ui.u(4.0),
        y + ui.u(5.0) - bounce,
        size,
        with_alpha(BLACK, 0.6),
        true,
        Align::Center,
    );
    ui.text(
        "BOMBERMAN",
        cx,
        y - bounce,
        size,
        HIGHLIGHT,
        true,
        Align::Center,
    );
    let w = ui.measure("BOMBERMAN", size, true);
    let frame = ((ui.time * 4.0) as usize) % 4;
    let b = size * 1.1;
    sprites.draw(
        &format!("bomb_{frame}"),
        cx + w / 2.0 + ui.u(8.0),
        y - size * 0.95,
        b,
        b,
        WHITE,
    );
    sprites.draw_flipped(
        &format!("bomb_{}", 3 - frame),
        cx - w / 2.0 - ui.u(8.0) - b,
        y - size * 0.95,
        b,
        b,
        WHITE,
    );
}

// -- connect menu ------------------------------------------------------------------

pub struct MenuState {
    pub name: TextField,
    pub address: TextField,
    pub focus: usize,
    pub bot: bool,
    pub error: Option<String>,
}

pub fn menu(ui: &Ui, sprites: &Sprites, m: &mut MenuState, sound: bool) -> Option<Click> {
    backdrop(ui, sprites);
    title(ui, sprites, screen_height() * 0.22);

    let w = ui.u(460.0).min(screen_width() - ui.u(40.0));
    let x = (screen_width() - w) / 2.0;
    let mut y = screen_height() * 0.32;
    let panel = Rect::new(x - ui.u(28.0), y, w + ui.u(56.0), ui.u(390.0));
    ui.panel(panel, with_alpha(PANEL, 0.94));
    y += ui.u(58.0);

    let field_h = ui.u(50.0);
    if m.name
        .draw(ui, Rect::new(x, y, w, field_h), "Dein Name", m.focus == 0)
    {
        m.focus = 0;
    }
    y += field_h + ui.u(46.0);
    if m.address.draw(
        ui,
        Rect::new(x, y, w, field_h),
        "Server (Adresse oder Adresse:Port)",
        m.focus == 1,
    ) {
        m.focus = 1;
    }
    y += field_h + ui.u(22.0);

    let mut click = None;
    let half = (w - ui.u(12.0)) / 2.0;
    let mode = if m.bot {
        "Bot spielt für mich"
    } else {
        "Ich spiele selbst"
    };
    if ui.button(Rect::new(x, y, half, ui.u(42.0)), mode, true, false) {
        click = Some(Click::ToggleBot);
    }
    let snd = if sound { "Ton: an" } else { "Ton: aus" };
    if ui.button(
        Rect::new(x + half + ui.u(12.0), y, half, ui.u(42.0)),
        snd,
        true,
        false,
    ) {
        click = Some(Click::ToggleSound);
    }
    y += ui.u(62.0);
    if ui.button(Rect::new(x, y, w, ui.u(58.0)), "VERBINDEN", true, true) {
        click = Some(Click::Connect);
    }
    y += ui.u(84.0);
    if let Some(error) = &m.error {
        let msg = ui.fit(error, ui.u(17.0), false, w + ui.u(40.0));
        ui.text_shadow(
            &msg,
            screen_width() / 2.0,
            y,
            ui.u(17.0),
            WARNING,
            false,
            Align::Center,
        );
    }
    ui.text(
        "Tab wechselt das Feld · Enter verbindet",
        screen_width() / 2.0,
        screen_height() - ui.u(24.0),
        ui.u(15.0),
        TEXT_DIM,
        false,
        Align::Center,
    );
    click
}

// -- waiting screens ---------------------------------------------------------------

pub fn waiting(
    ui: &Ui,
    sprites: &Sprites,
    headline: &str,
    detail: &str,
    hint: Option<&str>,
) -> Option<Click> {
    backdrop(ui, sprites);
    title(ui, sprites, screen_height() * 0.25);
    let cx = screen_width() / 2.0;
    let cy = screen_height() * 0.48;
    // A player walking in place: something alive while nothing happens.
    let frame = ((ui.time * 8.0) as usize) % 4;
    let s = ui.u(96.0);
    sprites.draw(
        &player_sprite(0, Direction::Down, frame),
        cx - s / 2.0,
        cy - s,
        s,
        s,
        WHITE,
    );
    ui.text_shadow(
        headline,
        cx,
        cy + ui.u(46.0),
        ui.u(30.0),
        TEXT,
        true,
        Align::Center,
    );
    ui.text(
        detail,
        cx,
        cy + ui.u(80.0),
        ui.u(18.0),
        TEXT_DIM,
        false,
        Align::Center,
    );
    if let Some(hint) = hint {
        ui.text(
            hint,
            cx,
            cy + ui.u(116.0),
            ui.u(16.0),
            WARNING,
            false,
            Align::Center,
        );
    }
    let bw = ui.u(220.0);
    if ui.button(
        Rect::new(cx - bw / 2.0, cy + ui.u(150.0), bw, ui.u(46.0)),
        "Zurück (Esc)",
        true,
        false,
    ) {
        return Some(Click::Disconnect);
    }
    None
}

// -- lobby -------------------------------------------------------------------------

pub struct LobbyView<'a> {
    pub session: &'a Session,
    pub bot: bool,
    pub sound: bool,
    /// When the countdown number last changed, for its pop-in animation.
    pub countdown_changed_at: f64,
}

pub fn lobby(ui: &Ui, sprites: &Sprites, v: &LobbyView) -> Option<Click> {
    backdrop(ui, sprites);
    title(ui, sprites, screen_height() * 0.16);
    let s = v.session;
    let cx = screen_width() / 2.0;
    let Some(lobby) = &s.lobby else {
        ui.text(
            "Warte auf den Server …",
            cx,
            screen_height() * 0.5,
            ui.u(22.0),
            TEXT_DIM,
            false,
            Align::Center,
        );
        return None;
    };
    let details = lobby.details.as_ref();

    ui.text(
        &format!(
            "Server {} · {} von {} Plätzen belegt",
            s.server_label(),
            lobby.players_connected,
            lobby.max_players
        ),
        cx,
        screen_height() * 0.16 + ui.u(44.0),
        ui.u(17.0),
        TEXT_DIM,
        false,
        Align::Center,
    );

    // Seat cards: a fixed grid, empty seats included, so nothing shifts.
    let seats = lobby.max_players.max(1) as usize;
    let columns = if screen_width() < ui.u(900.0) {
        2
    } else {
        seats.min(4)
    };
    let rows = seats.div_ceil(columns);
    let gap = ui.u(18.0);
    let card_w =
        ((screen_width() * 0.9 - gap * (columns as f32 - 1.0)) / columns as f32).min(ui.u(270.0));
    let card_h = ui.u(if rows > 1 { 150.0 } else { 220.0 });
    let total_w = card_w * columns as f32 + gap * (columns as f32 - 1.0);
    let top = screen_height() * 0.30;
    let can_bots = s.can_change_bots();
    let mut click = None;
    for i in 0..seats {
        let (col, row) = (i % columns, i / columns);
        let r = Rect::new(
            (screen_width() - total_w) / 2.0 + col as f32 * (card_w + gap),
            top + row as f32 * (card_h + gap),
            card_w,
            card_h,
        );
        let seat = details.and_then(|d| d.seats.get(i));
        let occupied = seat.map_or(lobby.slot_mask & (1 << i) != 0, |s| s.occupied);
        let server_bot = seat.is_some_and(|s| s.bot);
        if occupied {
            let name = seat
                .map(|s| s.name.clone())
                .unwrap_or_else(|| format!("bot-{i}"));
            let stale = seat.is_some_and(|s| s.stale);
            let status = if server_bot {
                SeatStatusLine::ServerBot
            } else if stale {
                SeatStatusLine::Stale
            } else if s.my_id == Some(i as u8) {
                SeatStatusLine::Me { bot: v.bot }
            } else {
                SeatStatusLine::Ready
            };
            seat_card(
                ui,
                sprites,
                r,
                i as u8,
                &name,
                s.my_id == Some(i as u8),
                status,
            );
            // A small corner button: the card itself belongs to the bot.
            let size = ui.u(30.0);
            let corner = Rect::new(r.x + r.w - size - ui.u(8.0), r.y + ui.u(8.0), size, size);
            if server_bot && can_bots && ui.button(corner, "X", true, false) {
                click = Some(Click::SeatBot {
                    seat: i as u8,
                    enabled: false,
                });
            }
        } else {
            rounded_outline(r, ui.u(14.0), ui.u(2.0), with_alpha(TEXT_DIM, 0.5));
            let label_y = if can_bots {
                r.y + r.h / 2.0 - ui.u(18.0)
            } else {
                r.y + r.h / 2.0 + ui.u(6.0)
            };
            ui.text(
                "Platz frei",
                r.x + r.w / 2.0,
                label_y,
                ui.u(18.0),
                TEXT_DIM,
                false,
                Align::Center,
            );
            if can_bots {
                let bw = (r.w - ui.u(32.0)).min(ui.u(200.0));
                let button =
                    Rect::new(r.x + (r.w - bw) / 2.0, label_y + ui.u(16.0), bw, ui.u(40.0));
                if ui.button(button, "+ Server-Bot", true, false) {
                    click = Some(Click::SeatBot {
                        seat: i as u8,
                        enabled: true,
                    });
                }
            }
        }
    }

    let below = top + rows as f32 * (card_h + gap) + ui.u(18.0);

    if lobby.state == LobbyState::Countdown {
        countdown(ui, lobby.countdown_ticks, v.countdown_changed_at);
    } else {
        let (hint, color) = lobby_hint(s);
        ui.text_shadow(
            &hint,
            cx,
            below + ui.u(10.0),
            ui.u(19.0),
            color,
            false,
            Align::Center,
        );
        let bw = ui.u(440.0).min(screen_width() - ui.u(40.0));
        if ui.button(
            Rect::new(cx - bw / 2.0, below + ui.u(32.0), bw, ui.u(62.0)),
            "MATCH STARTEN (Enter)",
            s.can_request_start(),
            true,
        ) {
            click = Some(Click::Start);
        }
    }

    // Footer buttons.
    let bw = ui.u(210.0);
    let by = screen_height() - ui.u(64.0);
    let bx = cx - bw * 1.5 - ui.u(12.0);
    let mode = if v.bot {
        "Bot spielt (B)"
    } else {
        "Selbst spielen (B)"
    };
    if ui.button(Rect::new(bx, by, bw, ui.u(42.0)), mode, true, false) {
        click = Some(Click::ToggleBot);
    }
    let snd = if v.sound { "Ton an (M)" } else { "Ton aus (M)" };
    if ui.button(
        Rect::new(bx + bw + ui.u(12.0), by, bw, ui.u(42.0)),
        snd,
        true,
        false,
    ) {
        click = Some(Click::ToggleSound);
    }
    if ui.button(
        Rect::new(bx + 2.0 * (bw + ui.u(12.0)), by, bw, ui.u(42.0)),
        "Trennen (Esc)",
        true,
        false,
    ) {
        click = Some(Click::Disconnect);
    }
    click
}

fn lobby_hint(s: &Session) -> (String, Color) {
    let Some(lobby) = &s.lobby else {
        return (String::new(), TEXT_DIM);
    };
    let Some(d) = &lobby.details else {
        return (
            "Dieser Server kennt keinen Start durch Spieler.".into(),
            TEXT_DIM,
        );
    };
    if d.paused {
        return ("Pausiert.".into(), WARNING);
    }
    if !d.player_control {
        return ("Das Match startet die Moderation.".into(), TEXT_DIM);
    }
    if lobby.state == LobbyState::Locked && !d.can_start {
        return ("Die Lobby ist gesperrt.".into(), TEXT_DIM);
    }
    if d.can_start {
        let stale = d.seats.iter().filter(|s| s.occupied && s.stale).count();
        if stale > 0 {
            return (
                format!("{stale} Spieler reagiert nicht. Trotzdem starten?"),
                WARNING,
            );
        }
        return ("Alle da? Jeder Spieler kann das Match starten.".into(), OK);
    }
    (
        format!(
            "Warte auf Mitspieler ({} von mindestens {})",
            lobby.players_connected, d.min_players
        ),
        TEXT_DIM,
    )
}

/// The line under a seat's name.
#[derive(Clone, Copy)]
enum SeatStatusLine {
    Me {
        bot: bool,
    },
    Ready,
    Stale,
    /// Played by the server itself.
    ServerBot,
}

fn seat_card(
    ui: &Ui,
    sprites: &Sprites,
    r: Rect,
    id: u8,
    name: &str,
    me: bool,
    status: SeatStatusLine,
) {
    let stale = matches!(status, SeatStatusLine::Stale);
    let color = player(id);
    ui.panel(
        r,
        Color::new(color.r * 0.30, color.g * 0.30, color.b * 0.34, 0.95),
    );
    rounded_outline(
        r,
        ui.u(14.0),
        ui.u(3.0),
        if stale { WARNING } else { color },
    );
    // Idle animation: alternate between the two standing frames.
    let frame = if ((ui.time * 1000.0 / 700.0) as i64 + id as i64) % 2 == 0 {
        0
    } else {
        2
    };
    let avatar = (r.h * 0.55).min(r.w * 0.5);
    let bounce = if me {
        (ui.time as f32 * 4.0).sin().abs() * ui.u(5.0)
    } else {
        0.0
    };
    sprites.draw(
        &player_sprite(id, Direction::Down, frame),
        r.x + r.w / 2.0 - avatar / 2.0,
        r.y + ui.u(12.0) - bounce,
        avatar,
        avatar,
        WHITE,
    );
    let size = ui.u(21.0);
    let fitted = ui.fit(name, size, true, r.w - ui.u(20.0));
    ui.text_shadow(
        &fitted,
        r.x + r.w / 2.0,
        r.y + avatar + ui.u(40.0),
        size,
        TEXT,
        true,
        Align::Center,
    );
    let status_y = r.y + avatar + ui.u(66.0);
    if let SeatStatusLine::ServerBot = status {
        ui.text(
            "Server-Bot",
            r.x + r.w / 2.0,
            status_y,
            ui.u(15.0),
            TEXT_DIM,
            false,
            Align::Center,
        );
    } else if stale {
        ui.text(
            "reagiert nicht",
            r.x + r.w / 2.0,
            status_y,
            ui.u(15.0),
            WARNING,
            false,
            Align::Center,
        );
    } else if let SeatStatusLine::Me { bot } = status {
        let label = if bot { "DU · BOT" } else { "DU" };
        let w = ui.measure(label, ui.u(14.0), true) + ui.u(18.0);
        let badge = Rect::new(
            r.x + r.w / 2.0 - w / 2.0,
            status_y - ui.u(17.0),
            w,
            ui.u(24.0),
        );
        ui.panel(badge, player_text(id));
        ui.text(
            label,
            r.x + r.w / 2.0,
            status_y,
            ui.u(14.0),
            BACKGROUND,
            true,
            Align::Center,
        );
    } else {
        ui.text(
            "bereit",
            r.x + r.w / 2.0,
            status_y,
            ui.u(15.0),
            OK,
            false,
            Align::Center,
        );
    }
}

/// The big 3 - 2 - 1, popping in on each new second.
fn countdown(ui: &Ui, ticks: u16, changed_at: f64) {
    let seconds = (ticks as u32).div_ceil(TICK_RATE as u32).max(1);
    let age = ((ui.time - changed_at) as f32).clamp(0.0, 1.0);
    let pop = 1.0 + 0.6 * (1.0 - (age * 4.0).min(1.0)).powi(2);
    let size = ui.u(150.0) * pop;
    let alpha = (1.0 - (age - 0.7).max(0.0) * 2.0).clamp(0.3, 1.0);
    let cx = screen_width() / 2.0;
    let cy = screen_height() * 0.86;
    draw_rectangle(
        0.0,
        0.0,
        screen_width(),
        screen_height(),
        with_alpha(BLACK, 0.35),
    );
    ui.text(
        "GLEICH GEHT'S LOS",
        cx,
        screen_height() * 0.62,
        ui.u(28.0),
        TEXT,
        true,
        Align::Center,
    );
    ui.text_shadow(
        &seconds.to_string(),
        cx,
        cy,
        size,
        with_alpha(HIGHLIGHT, alpha),
        true,
        Align::Center,
    );
}

// -- in-game HUD -------------------------------------------------------------------

pub struct HudView<'a> {
    pub session: &'a Session,
    pub bot: bool,
}

/// The player list on the left. Returns the width it used.
pub fn sidebar(ui: &Ui, sprites: &Sprites, v: &HudView, x: f32, y: f32, w: f32) {
    let s = v.session;
    let Some(world) = &s.world else {
        return;
    };
    ui.text(
        "SPIELER",
        x,
        y + ui.u(20.0),
        ui.u(16.0),
        TEXT_DIM,
        true,
        Align::Left,
    );
    let mut cy = y + ui.u(34.0);
    let card_h = ui.u(96.0);
    for p in world.players.values() {
        let r = Rect::new(x, cy, w, card_h);
        let me = s.my_id == Some(p.id);
        let color = player(p.id);
        let dim = if p.alive { 1.0 } else { 0.45 };
        ui.panel(
            r,
            Color::new(
                color.r * 0.25 + 0.05,
                color.g * 0.25 + 0.05,
                color.b * 0.25 + 0.07,
                0.95,
            ),
        );
        if me {
            rounded_outline(r, ui.u(10.0), ui.u(2.5), player_text(p.id));
        }
        let a = ui.u(58.0);
        sprites.draw(
            &player_sprite(p.id, Direction::Down, 0),
            r.x + ui.u(8.0),
            r.y + ui.u(8.0),
            a,
            a,
            Color::new(dim, dim, dim, if p.alive { 1.0 } else { 0.6 }),
        );
        if !p.alive {
            let (x0, y0) = (r.x + ui.u(14.0), r.y + ui.u(14.0));
            draw_line(
                x0,
                y0,
                x0 + a - ui.u(12.0),
                y0 + a - ui.u(12.0),
                ui.u(4.0),
                WARNING,
            );
            draw_line(
                x0 + a - ui.u(12.0),
                y0,
                x0,
                y0 + a - ui.u(12.0),
                ui.u(4.0),
                WARNING,
            );
        }
        let tx = r.x + a + ui.u(16.0);
        let name_size = ui.u(17.0);
        let tag = if me {
            if v.bot {
                " (Bot)"
            } else {
                " (Du)"
            }
        } else {
            ""
        };
        let label = ui.fit(
            &format!("{}{}", s.name_of(p.id), tag),
            name_size,
            true,
            r.x + r.w - tx - ui.u(8.0),
        );
        ui.text(
            &label,
            tx,
            r.y + ui.u(26.0),
            name_size,
            with_alpha(player_text(p.id), dim),
            true,
            Align::Left,
        );

        // Stats: bombs, flame, speed with their power-up icons.
        let icon = ui.u(24.0);
        let mut sx = tx - ui.u(4.0);
        let sy = r.y + ui.u(36.0);
        for (kind, value) in [
            (bomber_domain::game::PowerupKind::ExtraBomb, p.bombs_max),
            (bomber_domain::game::PowerupKind::Flame, p.flame),
            (bomber_domain::game::PowerupKind::Speed, p.speed),
        ] {
            sprites.draw(
                &item_sprite(kind, 0),
                sx,
                sy,
                icon,
                icon,
                Color::new(1.0, 1.0, 1.0, dim),
            );
            ui.text(
                &value.to_string(),
                sx + icon + ui.u(1.0),
                sy + icon * 0.75,
                ui.u(15.0),
                with_alpha(TEXT, dim),
                true,
                Align::Left,
            );
            sx += icon + ui.u(22.0);
        }
        let status = if p.alive {
            format!("{} Punkte", p.score)
        } else {
            format!("raus · {} Punkte", p.score)
        };
        ui.text(
            &status,
            tx,
            r.y + ui.u(84.0),
            ui.u(14.0),
            if p.alive { TEXT_DIM } else { WARNING },
            false,
            Align::Left,
        );
        cy += card_h + ui.u(10.0);
    }

    // Controls, for whoever has not played before.
    let lines = [
        "Pfeile / WASD  laufen",
        "Leertaste  Bombe",
        "B  Bot an/aus",
        "P  Pause",
        "M  Ton",
        "Esc  Menü",
    ];
    let mut hy = screen_height() - ui.u(18.0) - ui.u(20.0) * lines.len() as f32;
    if hy < cy + ui.u(10.0) {
        return;
    }
    for line in lines {
        hy += ui.u(20.0);
        ui.text(line, x, hy, ui.u(14.0), TEXT_DIM, false, Align::Left);
    }
}

/// Round clock and sudden-death warning above the board.
pub fn top_bar(ui: &Ui, s: &Session, cx: f32, y: f32) {
    let Some(world) = &s.world else {
        return;
    };
    let rules = s.rules();
    let tick = world.tick;
    let left_ticks = rules.round_time_ticks.saturating_sub(tick);
    let secs = left_ticks.div_ceil(TICK_RATE as u32);
    let clock = format!("{}:{:02}", secs / 60, secs % 60);
    let urgent = secs <= 10 && s.phase == crate::session::Phase::Playing;
    let size = ui.u(36.0)
        * if urgent {
            1.0 + 0.08 * (ui.time as f32 * 8.0).sin()
        } else {
            1.0
        };
    ui.text_shadow(
        &clock,
        cx,
        y,
        size,
        if urgent { WARNING } else { TEXT },
        true,
        Align::Center,
    );

    if rules.sudden_death_tick < rules.round_time_ticks {
        let until = rules.sudden_death_tick as i64 - tick as i64;
        let label_x = cx + ui.u(80.0);
        if until <= 0 {
            let pulse = 0.6 + 0.4 * (ui.time as f32 * 6.0).sin();
            ui.text_shadow(
                "SUDDEN DEATH",
                label_x,
                y - ui.u(4.0),
                ui.u(20.0),
                with_alpha(WARNING, pulse),
                true,
                Align::Left,
            );
        } else if until <= 20 * TICK_RATE as i64 {
            let secs = (until as u32).div_ceil(TICK_RATE as u32);
            ui.text_shadow(
                &format!("Sudden Death in {secs} s"),
                label_x,
                y - ui.u(4.0),
                ui.u(18.0),
                FIRE,
                true,
                Align::Left,
            );
        }
    }
}

// -- overlays ----------------------------------------------------------------------

pub fn pause_overlay(ui: &Ui, area: Rect) -> Option<Click> {
    draw_rectangle(area.x, area.y, area.w, area.h, with_alpha(BLACK, 0.55));
    let cx = area.x + area.w / 2.0;
    let cy = area.y + area.h * 0.45;
    ui.text_shadow("PAUSE", cx, cy, ui.u(72.0), TEXT, true, Align::Center);
    ui.text(
        "Das Match ist angehalten.",
        cx,
        cy + ui.u(40.0),
        ui.u(18.0),
        TEXT_DIM,
        false,
        Align::Center,
    );
    let bw = ui.u(260.0);
    if ui.button(
        Rect::new(cx - bw / 2.0, cy + ui.u(70.0), bw, ui.u(52.0)),
        "Fortsetzen (P)",
        true,
        true,
    ) {
        return Some(Click::Resume);
    }
    None
}

pub struct ResultView<'a> {
    pub session: &'a Session,
    pub area: Rect,
    /// Seconds since the result arrived, for the entrance animation.
    pub age: f32,
}

pub fn results(ui: &Ui, sprites: &Sprites, v: &ResultView) -> Option<Click> {
    let s = v.session;
    let a = v.area;
    let fade = (v.age * 3.0).min(1.0);
    draw_rectangle(a.x, a.y, a.w, a.h, with_alpha(BLACK, 0.68 * fade));
    let cx = a.x + a.w / 2.0;
    let mut y = a.y + a.h * 0.24;

    let (winner, reason, rows) = match &s.result {
        Some(end) => (
            end.winner.map(|w| w.raw()),
            match end.reason {
                EndReason::LastStanding => "Als Letzter übrig",
                EndReason::Timeout => "Zeit abgelaufen",
                EndReason::Aborted => "Match abgebrochen",
            },
            {
                let mut r: Vec<_> = end
                    .results
                    .iter()
                    .map(|r| (r.placement, r.id.raw(), r.score))
                    .collect();
                r.sort();
                r
            },
        ),
        None => (None, "Match beendet", Vec::new()),
    };

    // Slide in from above.
    y -= (1.0 - fade) * ui.u(40.0);
    match winner {
        Some(id) => {
            let name = s.name_of(id);
            let size = ui.fit_size(&name, true, ui.u(76.0), ui.u(28.0), a.w * 0.6);
            ui.text(
                "SIEGER",
                cx,
                y - size * 0.82 - ui.u(10.0),
                ui.u(24.0),
                TEXT_DIM,
                true,
                Align::Center,
            );
            ui.text_shadow(&name, cx, y, size, player_text(id), true, Align::Center);
            let av = ui.u(84.0);
            let hop = (ui.time as f32 * 6.0).sin().abs() * ui.u(10.0);
            let frame = ((ui.time * 6.0) as usize) % 4;
            let w = ui.measure(&name, size, true);
            sprites.draw(
                &player_sprite(id, Direction::Down, frame),
                cx - w / 2.0 - av - ui.u(10.0),
                y - av * 0.85 - hop,
                av,
                av,
                WHITE,
            );
            sprites.draw(
                &player_sprite(id, Direction::Down, (frame + 2) % 4),
                cx + w / 2.0 + ui.u(10.0),
                y - av * 0.85 - hop,
                av,
                av,
                WHITE,
            );
        }
        None => {
            ui.text_shadow(
                "UNENTSCHIEDEN",
                cx,
                y,
                ui.u(64.0),
                TEXT,
                true,
                Align::Center,
            );
        }
    }
    y += ui.u(40.0);
    ui.text(reason, cx, y, ui.u(20.0), TEXT_DIM, false, Align::Center);
    y += ui.u(44.0);

    let row_h = ui.u(40.0);
    let table_w = ui.u(440.0).min(a.w * 0.9);
    let tx = cx - table_w / 2.0;
    for (placement, id, score) in rows {
        let r = Rect::new(tx, y, table_w, row_h - ui.u(6.0));
        ui.panel(r, with_alpha(PANEL, 0.9));
        let size = ui.u(19.0);
        let base = r.y + r.h / 2.0 + size * 0.36;
        ui.text(
            &format!("{placement}."),
            r.x + ui.u(34.0),
            base,
            size,
            if placement == 1 { HIGHLIGHT } else { TEXT_DIM },
            true,
            Align::Right,
        );
        let icon = r.h;
        sprites.draw(
            &player_sprite(id, Direction::Down, 0),
            r.x + ui.u(42.0),
            r.y,
            icon,
            icon,
            WHITE,
        );
        let name = ui.fit(&s.name_of(id), size, true, table_w - ui.u(190.0));
        ui.text(
            &name,
            r.x + ui.u(48.0) + icon,
            base,
            size,
            player_text(id),
            true,
            Align::Left,
        );
        ui.text(
            &format!("{score}"),
            r.x + r.w - ui.u(16.0),
            base,
            size,
            TEXT,
            false,
            Align::Right,
        );
        y += row_h;
    }

    y += ui.u(16.0);
    let bw = ui.u(440.0).min(a.w * 0.9);
    let can = s.can_request_start();
    if ui.button(
        Rect::new(cx - bw / 2.0, y, bw, ui.u(58.0)),
        "NÄCHSTES MATCH (Enter)",
        can,
        true,
    ) {
        return Some(Click::Start);
    }
    if !can {
        let hint = if s.player_control() {
            "Zu wenige Spieler für ein neues Match."
        } else {
            "Das nächste Match startet die Moderation."
        };
        ui.text(
            hint,
            cx,
            y + ui.u(84.0),
            ui.u(16.0),
            TEXT_DIM,
            false,
            Align::Center,
        );
    }
    None
}

pub struct GameMenuView {
    pub playing: bool,
    pub paused: bool,
    pub bot: bool,
    pub sound: bool,
    pub confirm_abort: bool,
    pub player_control: bool,
}

pub fn game_menu(ui: &Ui, v: &GameMenuView) -> Option<Click> {
    draw_rectangle(
        0.0,
        0.0,
        screen_width(),
        screen_height(),
        with_alpha(BLACK, 0.6),
    );
    let w = ui.u(380.0);
    let bh = ui.u(50.0);
    let gap = ui.u(12.0);
    let mut items: Vec<(Click, String, bool)> =
        vec![(Click::CloseMenu, "Weiterspielen (Esc)".into(), true)];
    if v.playing && v.player_control {
        if v.paused {
            items.push((Click::Resume, "Fortsetzen".into(), true));
        } else {
            items.push((Click::Pause, "Pause für alle".into(), true));
        }
        let label = if v.confirm_abort {
            "Wirklich abbrechen?"
        } else {
            "Match abbrechen"
        };
        items.push((Click::Abort, label.into(), true));
    }
    items.push((
        Click::ToggleBot,
        if v.bot {
            "Bot: an".into()
        } else {
            "Bot: aus".into()
        },
        true,
    ));
    items.push((
        Click::ToggleSound,
        if v.sound {
            "Ton: an".into()
        } else {
            "Ton: aus".into()
        },
        true,
    ));
    items.push((Click::Disconnect, "Verbindung trennen".into(), true));

    let total_h = items.len() as f32 * (bh + gap) + ui.u(70.0);
    let x = (screen_width() - w) / 2.0;
    let mut y = (screen_height() - total_h) / 2.0;
    ui.panel(
        Rect::new(
            x - ui.u(24.0),
            y - ui.u(20.0),
            w + ui.u(48.0),
            total_h + ui.u(20.0),
        ),
        PANEL,
    );
    ui.text(
        "MENÜ",
        screen_width() / 2.0,
        y + ui.u(30.0),
        ui.u(28.0),
        TEXT,
        true,
        Align::Center,
    );
    y += ui.u(56.0);
    let mut click = None;
    for (c, label, enabled) in items {
        let primary = c == Click::CloseMenu;
        let danger = c == Click::Abort && v.confirm_abort;
        let r = Rect::new(x, y, w, bh);
        if danger {
            ui.panel(
                Rect::new(
                    r.x - ui.u(3.0),
                    r.y - ui.u(3.0),
                    r.w + ui.u(6.0),
                    r.h + ui.u(9.0),
                ),
                WARNING,
            );
        }
        if ui.button(r, &label, enabled, primary) {
            click = Some(c);
        }
        y += bh + gap;
    }
    click
}

/// A short notice at the top of the screen.
pub fn toast(ui: &Ui, text: &str, age: f32) {
    let alpha = (1.0 - (age - 1.6).max(0.0) / 0.4).clamp(0.0, 1.0);
    let size = ui.u(19.0);
    let w = ui.measure(text, size, true) + ui.u(36.0);
    let r = Rect::new((screen_width() - w) / 2.0, ui.u(14.0), w, ui.u(40.0));
    ui.panel(r, with_alpha(PANEL_LIGHT, 0.95 * alpha));
    ui.text(
        text,
        screen_width() / 2.0,
        r.y + r.h / 2.0 + size * 0.36,
        size,
        with_alpha(TEXT, alpha),
        true,
        Align::Center,
    );
}
