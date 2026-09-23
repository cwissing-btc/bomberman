//! Visual effects: particles, screen shake, floating labels.
//!
//! Purely decorative and entirely client-side. Effects are driven by the
//! [`FxEvent`]s the world reports, and live in board pixels (64 per cell), so
//! they scale with the board.

use macroquad::prelude::*;
use macroquad::rand::gen_range;

use bomber_domain::game::PowerupKind;

use crate::assets::{Sprites, TILE};
use crate::ui::palette;
use crate::world::FxEvent;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// Bright, fast, shrinks as it dies.
    Spark,
    /// Grey, slow, grows and fades.
    Smoke,
    /// Crate splinters: fall with gravity.
    Chip,
    /// Winner celebration.
    Confetti,
    /// A soft ring that expands and fades.
    Ring,
}

#[derive(Clone, Copy)]
struct Particle {
    pos: Vec2,
    vel: Vec2,
    life: f32,
    max_life: f32,
    size: f32,
    color: Color,
    kind: Kind,
}

struct FloatText {
    pos: Vec2,
    text: String,
    color: Color,
    life: f32,
}

struct Ghost {
    player: u8,
    pos: Vec2,
    life: f32,
}

struct CrateBreak {
    x: u8,
    y: u8,
    age: f32,
}

/// Duration of the crate-break animation.
const CRATE_BREAK_SECS: f32 = 0.26;
const GHOST_SECS: f32 = 1.4;

#[derive(Default)]
pub struct Fx {
    particles: Vec<Particle>,
    texts: Vec<FloatText>,
    ghosts: Vec<Ghost>,
    breaks: Vec<CrateBreak>,
    /// 0..1; the shake amplitude is trauma squared, so small bumps stay small.
    trauma: f32,
    /// Brief white-out after a big blast.
    flash: f32,
}

fn centre(x: u8, y: u8) -> Vec2 {
    vec2((x as f32 + 0.5) * TILE, (y as f32 + 0.5) * TILE)
}

impl Fx {
    pub fn clear(&mut self) {
        *self = Fx::default();
    }

    /// React to one event. `positions` gives where each player is drawn.
    pub fn on_event(&mut self, event: FxEvent) {
        match event {
            FxEvent::Explosion { x, y, arms } => {
                let reach = arms.iter().map(|&a| a as f32).sum::<f32>();
                self.trauma = (self.trauma + 0.28 + reach * 0.02).min(1.0);
                self.flash = (self.flash + 0.18).min(0.35);
                let c = centre(x, y);
                self.burst(c, 26, 1.0);
                self.ring(c, palette::HIGHLIGHT);
                // Sparks along each arm, so a long blast reads as long.
                let dirs = [
                    vec2(0.0, -1.0),
                    vec2(0.0, 1.0),
                    vec2(-1.0, 0.0),
                    vec2(1.0, 0.0),
                ];
                for (dir, &len) in dirs.iter().zip(arms.iter()) {
                    for r in 1..=len {
                        self.burst(c + *dir * r as f32 * TILE, 6, 0.6);
                    }
                }
            }
            FxEvent::CrateBroken { x, y } => {
                self.breaks.push(CrateBreak { x, y, age: 0.0 });
                let c = centre(x, y);
                for _ in 0..14 {
                    let a = gen_range(0.0, std::f32::consts::TAU);
                    let speed = gen_range(80.0, 260.0);
                    self.particles.push(Particle {
                        pos: c + vec2(gen_range(-16.0, 16.0), gen_range(-16.0, 16.0)),
                        vel: vec2(a.cos(), a.sin()) * speed - vec2(0.0, 120.0),
                        life: 0.0,
                        max_life: gen_range(0.5, 0.9),
                        size: gen_range(4.0, 9.0),
                        color: [
                            Color::new(0.58, 0.36, 0.19, 1.0),
                            Color::new(0.39, 0.23, 0.12, 1.0),
                            Color::new(0.72, 0.50, 0.28, 1.0),
                        ][gen_range(0, 3)],
                        kind: Kind::Chip,
                    });
                }
            }
            FxEvent::BombPlaced { x, y } => {
                let c = centre(x, y) + vec2(0.0, 22.0);
                for _ in 0..8 {
                    let dir = if gen_range(0, 2) == 0 { -1.0 } else { 1.0 };
                    self.particles.push(Particle {
                        pos: c,
                        vel: vec2(dir * gen_range(40.0, 110.0), gen_range(-40.0, -10.0)),
                        life: 0.0,
                        max_life: gen_range(0.3, 0.5),
                        size: gen_range(5.0, 9.0),
                        color: Color::new(0.8, 0.8, 0.75, 0.5),
                        kind: Kind::Smoke,
                    });
                }
            }
            FxEvent::PowerupTaken { player, x, y, kind } => {
                let c = centre(x, y);
                self.ring(c, palette::player_text(player));
                for _ in 0..16 {
                    let a = gen_range(0.0, std::f32::consts::TAU);
                    self.particles.push(Particle {
                        pos: c,
                        vel: vec2(a.cos(), a.sin()) * gen_range(60.0, 180.0),
                        life: 0.0,
                        max_life: gen_range(0.4, 0.7),
                        size: gen_range(3.0, 6.0),
                        color: palette::HIGHLIGHT,
                        kind: Kind::Spark,
                    });
                }
                let text = match kind {
                    PowerupKind::ExtraBomb => "+1 Bombe",
                    PowerupKind::Flame => "+1 Feuer",
                    PowerupKind::Speed => "+1 Tempo",
                };
                self.texts.push(FloatText {
                    pos: c - vec2(0.0, 24.0),
                    text: text.into(),
                    color: palette::player_text(player),
                    life: 0.0,
                });
            }
            FxEvent::PowerupBurned { x, y } => self.burst(centre(x, y), 8, 0.5),
            FxEvent::PlayerDied { player, x, y } => {
                let c = centre(x, y);
                self.trauma = (self.trauma + 0.3).min(1.0);
                self.ghosts.push(Ghost {
                    player,
                    pos: c,
                    life: 0.0,
                });
                for _ in 0..28 {
                    let a = gen_range(0.0, std::f32::consts::TAU);
                    self.particles.push(Particle {
                        pos: c,
                        vel: vec2(a.cos(), a.sin()) * gen_range(80.0, 280.0),
                        life: 0.0,
                        max_life: gen_range(0.5, 0.9),
                        size: gen_range(4.0, 8.0),
                        color: palette::player_text(player),
                        kind: Kind::Spark,
                    });
                }
            }
            FxEvent::WallClosed { x, y } => {
                self.trauma = (self.trauma + 0.05).min(1.0);
                let c = centre(x, y);
                for _ in 0..6 {
                    self.particles.push(Particle {
                        pos: c + vec2(gen_range(-28.0, 28.0), 28.0),
                        vel: vec2(gen_range(-50.0, 50.0), gen_range(-80.0, -20.0)),
                        life: 0.0,
                        max_life: gen_range(0.4, 0.7),
                        size: gen_range(6.0, 12.0),
                        color: Color::new(0.55, 0.57, 0.65, 0.6),
                        kind: Kind::Smoke,
                    });
                }
            }
        }
    }

    fn burst(&mut self, c: Vec2, count: usize, power: f32) {
        for _ in 0..count {
            let a = gen_range(0.0, std::f32::consts::TAU);
            let speed = gen_range(60.0, 340.0) * power;
            self.particles.push(Particle {
                pos: c + vec2(gen_range(-10.0, 10.0), gen_range(-10.0, 10.0)),
                vel: vec2(a.cos(), a.sin()) * speed,
                life: 0.0,
                max_life: gen_range(0.25, 0.6),
                size: gen_range(3.0, 8.0) * power.max(0.6),
                color: [palette::HIGHLIGHT, palette::FIRE, WHITE][gen_range(0, 3)],
                kind: Kind::Spark,
            });
        }
        for _ in 0..(count / 3).max(2) {
            self.particles.push(Particle {
                pos: c + vec2(gen_range(-20.0, 20.0), gen_range(-20.0, 20.0)),
                vel: vec2(gen_range(-30.0, 30.0), gen_range(-70.0, -20.0)),
                life: 0.0,
                max_life: gen_range(0.7, 1.3),
                size: gen_range(10.0, 18.0),
                color: Color::new(0.35, 0.33, 0.36, 0.55),
                kind: Kind::Smoke,
            });
        }
    }

    fn ring(&mut self, c: Vec2, color: Color) {
        self.particles.push(Particle {
            pos: c,
            vel: Vec2::ZERO,
            life: 0.0,
            max_life: 0.35,
            size: 60.0,
            color,
            kind: Kind::Ring,
        });
    }

    /// A shower of confetti across the board, for the winner.
    pub fn celebrate(&mut self, color: Color, board_w: f32) {
        for _ in 0..140 {
            let tint = [color, palette::HIGHLIGHT, WHITE, palette::ACCENT][gen_range(0, 4)];
            self.particles.push(Particle {
                pos: vec2(gen_range(0.0, board_w), gen_range(-200.0, -10.0)),
                vel: vec2(gen_range(-60.0, 60.0), gen_range(60.0, 200.0)),
                life: 0.0,
                max_life: gen_range(2.5, 4.5),
                size: gen_range(5.0, 10.0),
                color: tint,
                kind: Kind::Confetti,
            });
        }
    }

    pub fn update(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.life += dt;
            match p.kind {
                Kind::Chip => p.vel.y += 900.0 * dt,
                Kind::Confetti => {
                    p.vel.y = p.vel.y.min(160.0) + 30.0 * dt;
                    p.vel.x += (p.life * 5.0 + p.pos.y * 0.02).sin() * 60.0 * dt;
                }
                Kind::Spark => p.vel *= 1.0 - 3.0 * dt,
                Kind::Smoke => p.vel *= 1.0 - 1.5 * dt,
                Kind::Ring => {}
            }
            p.pos += p.vel * dt;
        }
        self.particles.retain(|p| p.life < p.max_life);
        for t in &mut self.texts {
            t.life += dt;
            t.pos.y -= 40.0 * dt;
        }
        self.texts.retain(|t| t.life < 1.1);
        for g in &mut self.ghosts {
            g.life += dt;
            g.pos.y -= 50.0 * dt;
        }
        self.ghosts.retain(|g| g.life < GHOST_SECS);
        for b in &mut self.breaks {
            b.age += dt;
        }
        self.breaks.retain(|b| b.age < CRATE_BREAK_SECS);
        self.trauma = (self.trauma - 1.6 * dt).max(0.0);
        self.flash = (self.flash - 2.5 * dt).max(0.0);
    }

    /// Screen offset for the camera shake, in board pixels.
    pub fn shake(&self, time: f64) -> Vec2 {
        let amount = self.trauma * self.trauma * 14.0;
        let t = time as f32;
        vec2(
            (t * 47.0).sin() * (t * 13.0).cos(),
            (t * 53.0).cos() * (t * 17.0).sin(),
        ) * amount
    }

    pub fn flash(&self) -> f32 {
        self.flash
    }

    /// The crate-break frame for a cell, if one is breaking there.
    pub fn crate_break_frame(&self, x: u8, y: u8) -> Option<usize> {
        self.breaks
            .iter()
            .find(|b| b.x == x && b.y == y)
            .map(|b| ((b.age / CRATE_BREAK_SECS) * 4.0).min(3.0) as usize)
    }

    /// Particles and ghosts, in board pixels. Called inside the board camera.
    pub fn draw(&self, sprites: &Sprites) {
        for g in &self.ghosts {
            let k = g.life / GHOST_SECS;
            let alpha = (1.0 - k).powf(1.5) * 0.7;
            let wobble = (g.life * 9.0).sin() * 6.0;
            sprites.draw(
                &crate::assets::player_sprite(g.player, bomber_domain::shared::Direction::Down, 0),
                g.pos.x - TILE / 2.0 + wobble,
                g.pos.y - TILE / 2.0,
                TILE,
                TILE,
                Color::new(1.0, 1.0, 1.0, alpha),
            );
        }
        for p in &self.particles {
            let k = p.life / p.max_life;
            match p.kind {
                Kind::Spark => {
                    let s = p.size * (1.0 - k);
                    draw_rectangle(
                        p.pos.x - s / 2.0,
                        p.pos.y - s / 2.0,
                        s,
                        s,
                        palette::with_alpha(p.color, 1.0 - k * 0.5),
                    );
                }
                Kind::Smoke => {
                    let s = p.size * (1.0 + k * 1.5);
                    draw_circle(
                        p.pos.x,
                        p.pos.y,
                        s / 2.0,
                        palette::with_alpha(p.color, p.color.a * (1.0 - k)),
                    );
                }
                Kind::Chip => {
                    draw_rectangle(
                        p.pos.x,
                        p.pos.y,
                        p.size,
                        p.size * 0.6,
                        palette::with_alpha(p.color, 1.0 - k * k),
                    );
                }
                Kind::Confetti => {
                    let w = p.size * (p.life * 8.0 + p.pos.x).sin().abs().max(0.2);
                    draw_rectangle(
                        p.pos.x - w / 2.0,
                        p.pos.y,
                        w,
                        p.size * 0.5,
                        palette::with_alpha(p.color, (1.0 - k).min(1.0)),
                    );
                }
                Kind::Ring => {
                    let r = p.size * (0.3 + k);
                    draw_circle_lines(
                        p.pos.x,
                        p.pos.y,
                        r,
                        6.0 * (1.0 - k) + 1.0,
                        palette::with_alpha(p.color, 0.8 * (1.0 - k)),
                    );
                }
            }
        }
    }

    /// Floating labels, drawn in screen space so the text stays crisp.
    pub fn floating_texts(&self) -> impl Iterator<Item = (Vec2, &str, Color)> {
        self.texts.iter().map(|t| {
            let alpha = (1.0 - (t.life - 0.6).max(0.0) / 0.5).clamp(0.0, 1.0);
            (t.pos, t.text.as_str(), palette::with_alpha(t.color, alpha))
        })
    }
}
