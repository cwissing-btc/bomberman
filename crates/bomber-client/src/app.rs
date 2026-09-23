//! The application: menu or game, input, the send cadence, and composing the
//! frame.

use macroquad::prelude::*;

use bomber_domain::shared::Direction;
use bomber_protocol::{Action, TICK_RATE};

use crate::assets::{Fonts, Sprites, TILE};
use crate::board::{self, BoardView};
use crate::bot::{Bot, Situation};
use crate::fx::Fx;
use crate::input::Steering;
use crate::net::Link;
use crate::screens::{self, Click, GameMenuView, HudView, LobbyView, MenuState, ResultView};
use crate::session::{Cue, Phase, Session};
use crate::settings::Settings;
use crate::sound::{Sfx, Sounds};
use crate::ui::{palette, rounded_rect, TextField, Ui};
use crate::world::FxEvent;

const TICK: f64 = 1.0 / TICK_RATE as f64;
/// With `--autostart`, results stay up this long before the next match.
const AUTOSTART_RESULT_SECS: f64 = 6.0;
/// Hint after this long without a seat.
const SLOW_CONNECT_SECS: f64 = 4.0;

pub struct Options {
    pub settings: Settings,
    /// Skip the menu and connect at once.
    pub connect: bool,
    /// Request Start whenever it is possible, for unattended bot rounds.
    pub autostart: bool,
    /// Start with the F3 overlay on.
    pub debug: bool,
}

enum Screen {
    Menu(MenuState),
    Game(Box<Game>),
}

struct Game {
    session: Session,
    bot: Bot,
    steering: Steering,
    fx: Fx,
    menu_open: bool,
    confirm_abort: bool,
    tick_acc: f64,
    board_rt: Option<(RenderTarget, u8, u8)>,
    result_at: Option<f64>,
    countdown_value: Option<u16>,
    countdown_changed_at: f64,
}

pub struct App {
    settings: Settings,
    sprites: Sprites,
    fonts: Fonts,
    sounds: Sounds,
    screen: Screen,
    toast: Option<(String, f64)>,
    autostart: bool,
    last_autostart: f64,
    /// F3: frame rate, tick and phase in a corner.
    debug: bool,
}

impl App {
    pub async fn new(options: Options) -> App {
        let sounds = Sounds::load(options.settings.sound).await;
        let mut app = App {
            sprites: Sprites::load(),
            fonts: Fonts::load(),
            sounds,
            screen: Screen::Menu(menu_state(&options.settings, None)),
            settings: options.settings,
            toast: None,
            autostart: options.autostart,
            last_autostart: 0.0,
            debug: options.debug,
        };
        if options.connect {
            app.connect();
        }
        app
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), get_time()));
    }

    fn connect(&mut self) {
        let Screen::Menu(menu) = &mut self.screen else {
            return;
        };
        let name = menu.name.value.trim().to_string();
        if name.is_empty() {
            menu.error = Some("Bitte gib einen Namen ein.".into());
            return;
        }
        self.settings.name = name.clone();
        self.settings.set_address(&menu.address.value);
        self.settings.bot = menu.bot;
        self.settings.save();
        match Link::open(&self.settings.host, self.settings.port) {
            Ok(link) => {
                let now = get_time();
                self.screen = Screen::Game(Box::new(Game {
                    session: Session::new(link, name, now),
                    bot: Bot::default(),
                    steering: Steering::default(),
                    fx: Fx::default(),
                    menu_open: false,
                    confirm_abort: false,
                    tick_acc: 0.0,
                    board_rt: None,
                    result_at: None,
                    countdown_value: None,
                    countdown_changed_at: now,
                }));
            }
            Err(e) => {
                menu.error = Some(format!(
                    "„{}“ ist nicht erreichbar: {e}",
                    self.settings.host
                ));
            }
        }
    }

    fn disconnect(&mut self) {
        self.screen = Screen::Menu(menu_state(&self.settings, None));
    }

    fn toggle_bot(&mut self) {
        self.settings.bot = !self.settings.bot;
        self.settings.save();
        match &mut self.screen {
            Screen::Menu(m) => m.bot = self.settings.bot,
            Screen::Game(g) => {
                g.steering.clear();
                g.bot.reset();
            }
        }
        let text = if self.settings.bot {
            "Bot spielt für dich"
        } else {
            "Du spielst selbst"
        };
        self.toast(text);
    }

    fn toggle_sound(&mut self) {
        self.settings.sound = !self.settings.sound;
        self.sounds.enabled = self.settings.sound;
        self.settings.save();
        self.sounds.play(Sfx::Click);
    }

    pub fn frame(&mut self) {
        let now = get_time();
        let dt = get_frame_time().min(0.1);

        if is_key_pressed(KeyCode::F11) {
            toggle_fullscreen();
        }
        if is_key_pressed(KeyCode::F3) {
            self.debug = !self.debug;
        }

        let click = match &mut self.screen {
            Screen::Menu(_) => self.menu_frame(),
            Screen::Game(_) => self.game_frame(now, dt),
        };
        if let Some(click) = click {
            self.on_click(click);
        }

        if self.debug {
            let line = match &self.screen {
                Screen::Game(g) => format!(
                    "{} fps · Phase {:?} · Tick {} · Spieler {:?}",
                    get_fps(),
                    g.session.phase,
                    g.session.at_tick.map_or("-".into(), |t| t.to_string()),
                    g.session.my_id
                ),
                Screen::Menu(_) => format!("{} fps", get_fps()),
            };
            let ui = Ui::new(&self.fonts, now);
            ui.text_shadow(
                &line,
                screen_width() - ui.u(12.0),
                screen_height() - ui.u(10.0),
                ui.u(14.0),
                palette::TEXT,
                false,
                crate::ui::Align::Right,
            );
            if std::env::var_os("BOMBER_DEBUG_LOG").is_some() && (now * 2.0).fract() < 0.02 {
                eprintln!("{line}");
            }
        }

        if let Some((text, at)) = &self.toast {
            let age = (now - at) as f32;
            if age > 2.0 {
                self.toast = None;
            } else {
                let ui = Ui::new(&self.fonts, now);
                screens::toast(&ui, text, age);
            }
        }
    }

    fn on_click(&mut self, click: Click) {
        match click {
            Click::Connect => self.connect(),
            Click::ToggleBot => self.toggle_bot(),
            Click::ToggleSound => self.toggle_sound(),
            Click::Disconnect => self.disconnect(),
            Click::CloseMenu => {
                if let Screen::Game(g) = &mut self.screen {
                    g.menu_open = false;
                    g.confirm_abort = false;
                }
            }
            Click::SeatBot { seat, enabled } => self.request_seat_bot(seat, enabled),
            Click::Start => self.request(Action::Start),
            Click::Pause => self.request(Action::Pause),
            Click::Resume => self.request(Action::Resume),
            Click::Abort => {
                let Screen::Game(g) = &mut self.screen else {
                    return;
                };
                if g.confirm_abort {
                    g.confirm_abort = false;
                    g.menu_open = false;
                    self.request(Action::Abort);
                } else {
                    g.confirm_abort = true;
                }
            }
        }
    }

    fn request_seat_bot(&mut self, seat: u8, enabled: bool) {
        let Screen::Game(g) = &mut self.screen else {
            return;
        };
        if !g.session.can_change_bots() {
            self.toast("Bots lassen sich gerade nicht ändern.");
            return;
        }
        g.session.request_seat_bot(seat, enabled);
    }

    fn request(&mut self, action: Action) {
        let Screen::Game(g) = &mut self.screen else {
            return;
        };
        if !g.session.player_control() {
            self.toast("Auf diesem Server steuert nur die Moderation das Match.");
            return;
        }
        if action == Action::Start && !g.session.can_request_start() {
            self.toast("Start ist gerade nicht möglich.");
            return;
        }
        g.session.request(action);
        if action == Action::Pause || action == Action::Resume {
            g.menu_open = false;
        }
        self.sounds.play(Sfx::Click);
    }

    // -- menu ----------------------------------------------------------------

    fn menu_frame(&mut self) -> Option<Click> {
        let Screen::Menu(menu) = &mut self.screen else {
            return None;
        };
        let mut typed = Vec::new();
        while let Some(c) = get_char_pressed() {
            typed.push(c);
        }
        if is_key_pressed(KeyCode::Tab) {
            menu.focus = (menu.focus + 1) % 2;
        }
        let field = if menu.focus == 0 {
            &mut menu.name
        } else {
            &mut menu.address
        };
        field.edit(&typed);

        let ui = Ui::new(&self.fonts, get_time());
        let click = screens::menu(&ui, &self.sprites, menu, self.settings.sound);
        if is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter) {
            return Some(Click::Connect);
        }
        click
    }

    // -- game ----------------------------------------------------------------

    fn game_frame(&mut self, now: f64, dt: f32) -> Option<Click> {
        let bot_on = self.settings.bot;
        let Screen::Game(g) = &mut self.screen else {
            return None;
        };

        // Keys that work everywhere in a game.
        let mut click = None;
        if is_key_pressed(KeyCode::Escape) {
            click = Some(match g.session.phase {
                Phase::Connecting | Phase::Lost => Click::Disconnect,
                Phase::Lobby => Click::Disconnect,
                _ if g.menu_open => Click::CloseMenu,
                _ => {
                    g.menu_open = true;
                    g.confirm_abort = false;
                    return None;
                }
            });
        }
        if is_key_pressed(KeyCode::B) {
            click = Some(Click::ToggleBot);
        }
        if is_key_pressed(KeyCode::M) {
            click = Some(Click::ToggleSound);
        }
        if !g.menu_open {
            let start_key = is_key_pressed(KeyCode::Enter) || is_key_pressed(KeyCode::KpEnter);
            match g.session.phase {
                Phase::Lobby | Phase::MatchOver if start_key => click = Some(Click::Start),
                Phase::Playing if is_key_pressed(KeyCode::P) => {
                    click = Some(if g.session.paused() {
                        Click::Resume
                    } else {
                        Click::Pause
                    });
                }
                _ => {}
            }
        }

        // Steering keys: tracked always, used only when playing by hand.
        for (keys, dir) in [
            ([KeyCode::Up, KeyCode::W], Direction::Up),
            ([KeyCode::Down, KeyCode::S], Direction::Down),
            ([KeyCode::Left, KeyCode::A], Direction::Left),
            ([KeyCode::Right, KeyCode::D], Direction::Right),
        ] {
            if keys.iter().any(|k| is_key_pressed(*k)) {
                g.steering.press(dir);
            }
            if keys.iter().any(|k| is_key_released(*k)) && !keys.iter().any(|k| is_key_down(*k)) {
                g.steering.release(dir);
            }
        }
        if is_key_pressed(KeyCode::Space) {
            g.steering.bomb();
        }

        // Network in, then cues.
        g.session.pump(now);
        let cues: Vec<Cue> = std::mem::take(&mut g.session.cues);
        let mut exploded = false;
        for cue in cues {
            match cue {
                Cue::Fx(event) => {
                    g.fx.on_event(event);
                    match event {
                        FxEvent::Explosion { .. } if !exploded => {
                            exploded = true;
                            self.sounds.play_at(Sfx::Explosion, 0.8);
                        }
                        FxEvent::BombPlaced { .. } => self.sounds.play_at(Sfx::BombPlaced, 0.5),
                        FxEvent::PowerupTaken { player, .. } => {
                            let mine = g.session.my_id == Some(player);
                            self.sounds
                                .play_at(Sfx::Pickup, if mine { 1.0 } else { 0.35 });
                        }
                        FxEvent::PlayerDied { .. } => self.sounds.play(Sfx::Death),
                        _ => {}
                    }
                }
                Cue::MatchStarted => {
                    g.fx.clear();
                    g.bot.reset();
                    g.result_at = None;
                    self.sounds.play(Sfx::Go);
                }
                Cue::MatchEnded => {
                    g.result_at = Some(now);
                    self.sounds.play(Sfx::Win);
                }
                Cue::CountdownSecond(_) => self.sounds.play(Sfx::Beep),
            }
        }
        if g.session.phase == Phase::MatchOver && g.result_at.is_none() {
            g.result_at = Some(now);
        }
        if let Some(end) = &g.session.result {
            // Confetti once, in the winner's colour.
            if g.result_at.is_some_and(|t| now - t < 0.05) {
                if let (Some(w), Some(world)) = (end.winner, &g.session.world) {
                    g.fx.celebrate(palette::player_text(w.raw()), world.width as f32 * TILE);
                }
            }
        }

        // At most one packet per frame and 60 per second. Never several at
        // once to catch up: the server keeps only the newest packet of a tick
        // window, so a burst would let a later move overwrite a bomb.
        g.tick_acc += dt as f64;
        if g.tick_acc >= TICK {
            g.tick_acc = (g.tick_acc - TICK).min(TICK);
            let playing = g.session.phase == Phase::Playing && !g.session.paused();
            let action = match (&g.session.world, g.session.my_id) {
                (Some(world), Some(my_id)) if playing && bot_on => g.bot.decide(&Situation {
                    world,
                    rules: g.session.rules(),
                    my_id,
                    at_tick: g.session.at_tick,
                    match_id: g.session.init.as_ref().map(|i| i.match_id),
                    now,
                }),
                _ if playing && !g.menu_open => g.steering.action(),
                _ => Action::Idle,
            };
            g.session.send_tick(action, now);
        }
        g.fx.update(dt);

        // Countdown number changes drive the pop-in.
        let countdown = g
            .session
            .lobby
            .as_ref()
            .map(|l| l.countdown_ticks.div_ceil(60));
        if countdown != g.countdown_value {
            g.countdown_value = countdown;
            g.countdown_changed_at = now;
        }

        // Unattended: start whenever possible, leaving results up for a moment.
        if self.autostart && click.is_none() && now - self.last_autostart > 2.0 {
            let results_read = g.result_at.is_none_or(|t| now - t > AUTOSTART_RESULT_SECS);
            if g.session.can_request_start() && results_read {
                self.last_autostart = now;
                click = Some(Click::Start);
            }
        }

        let drawn = draw_game(g, &self.sprites, &self.fonts, &self.settings, now);
        click.or(drawn)
    }
}

fn menu_state(settings: &Settings, error: Option<String>) -> MenuState {
    MenuState {
        name: TextField::new(&settings.name, 24),
        address: TextField::new(&settings.address(), 64),
        focus: if settings.name.is_empty() { 0 } else { 1 },
        bot: settings.bot,
        error,
    }
}

fn draw_game(
    g: &mut Game,
    sprites: &Sprites,
    fonts: &Fonts,
    settings: &Settings,
    now: f64,
) -> Option<Click> {
    let ui = Ui::new(fonts, now);
    let s = &g.session;
    match s.phase {
        Phase::Connecting => {
            let waited = s.seconds_connecting(now);
            let hint = (waited > SLOW_CONNECT_SECS).then(|| {
                format!(
                    "Keine Antwort. Server aus, UDP-Port {} zu, Lobby voll oder ein Match läuft gerade?",
                    settings.port
                )
            });
            return screens::waiting(
                &ui,
                sprites,
                "Verbinde …",
                &s.server_label(),
                hint.as_deref(),
            );
        }
        Phase::Lost => {
            return screens::waiting(
                &ui,
                sprites,
                "Verbindung verloren",
                "Versuche es weiter …",
                Some("Der Server antwortet seit über 3 Sekunden nicht."),
            );
        }
        Phase::Lobby => {
            return screens::lobby(
                &ui,
                sprites,
                &LobbyView {
                    session: s,
                    bot: settings.bot,
                    sound: settings.sound,
                    countdown_changed_at: g.countdown_changed_at,
                },
            );
        }
        Phase::Playing | Phase::MatchOver => {}
    }

    clear_background(palette::BACKGROUND);
    let sidebar_w = ui.u(250.0).min(screen_width() * 0.3);
    let pad = ui.u(16.0);
    let top = ui.u(58.0);
    let area = Rect::new(
        sidebar_w + pad * 2.0,
        top,
        screen_width() - sidebar_w - pad * 3.0,
        screen_height() - top - pad,
    );

    let mut click = None;
    if let Some(world) = &s.world {
        let (bw, bh) = (world.width, world.height);
        let (pw, ph) = (bw as f32 * TILE, bh as f32 * TILE);
        if g.board_rt
            .as_ref()
            .is_none_or(|(_, w, h)| (*w, *h) != (bw, bh))
        {
            let rt = render_target(pw as u32, ph as u32);
            g.board_rt = Some((rt, bw, bh));
        }
        let (rt, _, _) = g.board_rt.as_ref().unwrap();

        let sub_tick = if s.phase == Phase::Playing {
            s.sub_tick(now)
        } else {
            0.0
        };
        let rules = s.rules();

        // Board into its own texture at native 64 px per cell...
        let mut camera = Camera2D::from_display_rect(Rect::new(0.0, 0.0, pw, ph));
        // from_display_rect flips y for the screen; a render target is
        // already stored bottom-up, so flip it back or the board is upside down.
        camera.zoom.y = -camera.zoom.y;
        camera.render_target = Some(rt.clone());
        set_camera(&camera);
        clear_background(BLACK);
        board::draw(&BoardView {
            world,
            rules,
            my_id: s.my_id,
            sub_tick,
            time: now,
            sprites,
            fx: &g.fx,
        });
        set_default_camera();

        // ...then scaled into the window. Enlarging stays crisp; shrinking
        // is filtered so thin lines do not flicker.
        let scale = (area.w / pw).min(area.h / ph);
        rt.texture.set_filter(if scale >= 1.0 {
            FilterMode::Nearest
        } else {
            FilterMode::Linear
        });
        let (dw, dh) = (pw * scale, ph * scale);
        let shake = g.fx.shake(now) * scale;
        let origin = vec2(area.x + (area.w - dw) / 2.0, area.y + (area.h - dh) / 2.0);
        rounded_rect(
            Rect::new(
                origin.x - ui.u(6.0),
                origin.y - ui.u(6.0),
                dw + ui.u(12.0),
                dh + ui.u(12.0),
            ),
            ui.u(10.0),
            palette::PANEL_LIGHT,
        );
        draw_texture_ex(
            &rt.texture,
            origin.x + shake.x,
            origin.y + shake.y,
            WHITE,
            DrawTextureParams {
                dest_size: Some(vec2(dw, dh)),
                ..Default::default()
            },
        );
        let flash = g.fx.flash();
        if flash > 0.0 {
            draw_rectangle(
                origin.x,
                origin.y,
                dw,
                dh,
                Color::new(1.0, 0.95, 0.8, flash * 0.5),
            );
        }

        // Names in screen space, crisp at any zoom.
        let tag = (TILE * scale * 0.24).clamp(11.0, 22.0);
        for (id, c) in board::player_centres(world, &rules, sub_tick) {
            let p = origin + shake + c * scale;
            let name = ui.fit(&s.name_of(id), tag, true, TILE * scale * 2.6);
            // Our own name sits above the marker arrow.
            let lift = if s.my_id == Some(id) {
                tag * 0.4 + 26.0 * scale
            } else {
                tag * 0.2
            };
            let y = p.y - TILE * scale * 0.5 - lift;
            ui.text_shadow(
                &name,
                p.x,
                y,
                tag,
                palette::player_text(id),
                true,
                crate::ui::Align::Center,
            );
        }
        for (pos, text, color) in g.fx.floating_texts() {
            let p = origin + shake + pos * scale;
            ui.text_shadow(
                text,
                p.x,
                p.y,
                tag * 1.1,
                color,
                true,
                crate::ui::Align::Center,
            );
        }

        let board_rect = Rect::new(origin.x, origin.y, dw, dh);
        screens::top_bar(&ui, s, board_rect.x + board_rect.w / 2.0, top - ui.u(14.0));

        if s.phase == Phase::Playing && s.paused() && !g.menu_open {
            click = screens::pause_overlay(&ui, board_rect);
        }
        if s.phase == Phase::MatchOver && !g.menu_open {
            let age = g.result_at.map_or(0.0, |t| (now - t) as f32);
            click = screens::results(
                &ui,
                sprites,
                &ResultView {
                    session: s,
                    area: board_rect,
                    age,
                },
            );
        }
    } else if s.phase == Phase::MatchOver && !g.menu_open {
        // Joined after the match: results without a board beneath.
        let age = g.result_at.map_or(0.0, |t| (now - t) as f32);
        let full = Rect::new(0.0, 0.0, screen_width(), screen_height());
        click = screens::results(
            &ui,
            sprites,
            &ResultView {
                session: s,
                area: full,
                age,
            },
        );
    } else {
        ui.text(
            "Warte auf das Spielfeld …",
            area.x + area.w / 2.0,
            area.y + area.h / 2.0,
            ui.u(22.0),
            palette::TEXT_DIM,
            false,
            crate::ui::Align::Center,
        );
    }

    screens::sidebar(
        &ui,
        sprites,
        &HudView {
            session: s,
            bot: settings.bot,
        },
        pad,
        pad,
        sidebar_w,
    );

    if g.menu_open {
        click = screens::game_menu(
            &ui,
            &GameMenuView {
                playing: s.phase == Phase::Playing,
                paused: s.paused(),
                bot: settings.bot,
                sound: settings.sound,
                confirm_abort: g.confirm_abort,
                player_control: s.player_control(),
            },
        );
    }
    click
}

fn toggle_fullscreen() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FULL: AtomicBool = AtomicBool::new(false);
    let full = !FULL.load(Ordering::Relaxed);
    FULL.store(full, Ordering::Relaxed);
    set_fullscreen(full);
}
