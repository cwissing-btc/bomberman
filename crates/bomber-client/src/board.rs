//! Draws the board, in board pixels (64 per cell).
//!
//! Layer order: floor, sudden-death warning, blocks, power-ups, bombs, flames,
//! players (sorted by y, so a lower figure overlaps a higher one), effects.
//! Names are drawn later in screen space by the caller, so text stays crisp at
//! any zoom.

use std::collections::HashSet;

use macroquad::prelude::*;

use bomber_domain::board::Tile;
use bomber_domain::game::Rules;

use crate::assets::{item_sprite, player_sprite, Sprites, TILE};
use crate::fx::Fx;
use crate::ui::palette;
use crate::world::{World, SUDDEN_DEATH_TICKS_PER_CELL};

pub struct BoardView<'a> {
    pub world: &'a World,
    pub rules: Rules,
    pub my_id: Option<u8>,
    pub sub_tick: f32,
    pub time: f64,
    pub sprites: &'a Sprites,
    pub fx: &'a Fx,
}

/// Which explosion piece a burning cell shows, derived from its burning
/// neighbours. The server only lists burning cells; the sprite set splits a
/// blast into centre, arms and tips so any length can be built from them.
pub fn explosion_piece(x: i32, y: i32, burning: &HashSet<(i32, i32)>) -> &'static str {
    let up = burning.contains(&(x, y - 1));
    let down = burning.contains(&(x, y + 1));
    let left = burning.contains(&(x - 1, y));
    let right = burning.contains(&(x + 1, y));
    let horizontal = left || right;
    let vertical = up || down;
    match (horizontal, vertical) {
        (true, true) => "expl_center",
        (true, false) if left && right => "expl_arm_h",
        // The tip points away from its neighbour: it is the end of the arm.
        (true, false) if left => "expl_tip_right",
        (true, false) => "expl_tip_left",
        (false, true) if up && down => "expl_arm_v",
        (false, true) if up => "expl_tip_down",
        (false, true) => "expl_tip_up",
        (false, false) => "expl_center",
    }
}

/// Spread progress 0..1 over `count` frames; 1.0 lands on the last one.
fn frame_for(progress: f32, count: usize) -> usize {
    ((progress.clamp(0.0, 1.0) * count as f32) as usize).min(count - 1)
}

/// Screen-independent centre of each living player, in board pixels.
pub fn player_centres(world: &World, rules: &Rules, sub_tick: f32) -> Vec<(u8, Vec2)> {
    world
        .players
        .values()
        .filter(|p| p.alive)
        .map(|p| {
            let (x, y) = World::visual_position(p, rules, sub_tick);
            (p.id, vec2((x + 0.5) * TILE, (y + 0.5) * TILE))
        })
        .collect()
}

pub fn draw(v: &BoardView) {
    let w = v.world;
    let t = v.time as f32;
    let now_tick = w.tick as f32 + v.sub_tick;

    // Floor.
    for y in 0..w.height {
        for x in 0..w.width {
            let name = if (x + y) % 2 == 0 {
                "floor_0"
            } else {
                "floor_1"
            };
            v.sprites
                .draw(name, x as f32 * TILE, y as f32 * TILE, TILE, TILE, WHITE);
        }
    }

    draw_sudden_death_warning(v, now_tick, t);

    // Blocks. A breaking crate sits on a cell that is already empty.
    for y in 0..w.height {
        for x in 0..w.width {
            let (px, py) = (x as f32 * TILE, y as f32 * TILE);
            if let Some(frame) = v.fx.crate_break_frame(x, y) {
                v.sprites
                    .draw(&format!("crate_break_{frame}"), px, py, TILE, TILE, WHITE);
                continue;
            }
            match w.tile(x as i32, y as i32) {
                Tile::Solid => v.sprites.draw("wall_solid", px, py, TILE, TILE, WHITE),
                Tile::Soft => v.sprites.draw("crate", px, py, TILE, TILE, WHITE),
                Tile::Empty => {}
            }
        }
    }

    // Power-ups: a gentle hover and a glow, so they read as "take me".
    let item_frame = ((v.time * 1000.0 / 320.0) as usize) % 2;
    for p in w.powerups.values() {
        let bob = (t * 3.0 + p.id as f32).sin() * 3.0;
        let (cx, cy) = ((p.x as f32 + 0.5) * TILE, (p.y as f32 + 0.5) * TILE);
        let glow = 0.18 + 0.08 * (t * 4.0 + p.id as f32).sin();
        draw_circle(
            cx,
            cy + 4.0,
            26.0,
            palette::with_alpha(palette::HIGHLIGHT, glow),
        );
        draw_ellipse(
            cx,
            cy + 26.0,
            16.0,
            5.0,
            0.0,
            Color::new(0.0, 0.0, 0.0, 0.25),
        );
        v.sprites.draw(
            &item_sprite(p.kind, item_frame),
            p.x as f32 * TILE,
            p.y as f32 * TILE + bob,
            TILE,
            TILE,
            WHITE,
        );
    }

    // Bombs: the fuse burns down through the frames, and the bomb swells
    // faster and flashes red as it nears zero.
    let fuse_total = v.rules.bomb_fuse_ticks.max(1) as f32;
    for b in w.bombs.values() {
        let left = (w.fuse_left(b, w.tick) as f32 - v.sub_tick).max(0.0);
        let urgency = 1.0 - (left / fuse_total).clamp(0.0, 1.0);
        let frame = frame_for(urgency, 4);
        let pulse = 1.0 + (0.04 + 0.08 * urgency) * (t * (6.0 + 22.0 * urgency)).sin();
        let size = TILE * pulse;
        let (cx, cy) = ((b.x as f32 + 0.5) * TILE, (b.y as f32 + 0.5) * TILE);
        draw_ellipse(
            cx,
            cy + 24.0,
            20.0 * pulse,
            6.0,
            0.0,
            Color::new(0.0, 0.0, 0.0, 0.3),
        );
        let tint = if urgency > 0.7 && (t * 14.0).sin() > 0.0 {
            Color::new(1.0, 0.55, 0.55, 1.0)
        } else {
            WHITE
        };
        v.sprites.draw(
            &format!("bomb_{frame}"),
            cx - size / 2.0,
            cy - size / 2.0,
            size,
            size,
            tint,
        );
    }

    // Flames: glow underneath first, then the pieces.
    let burning: HashSet<(i32, i32)> = w
        .flames
        .keys()
        .map(|&(x, y)| (x as i32, y as i32))
        .collect();
    let flame_total = v.rules.flame_duration_ticks.max(1) as f32;
    for f in w.flames.values() {
        let left = w.flame_left(f, w.tick) as f32 / flame_total;
        let (cx, cy) = ((f.x as f32 + 0.5) * TILE, (f.y as f32 + 0.5) * TILE);
        draw_circle(
            cx,
            cy,
            44.0,
            Color::new(1.0, 0.75, 0.25, 0.22 * left.max(0.3)),
        );
    }
    for f in w.flames.values() {
        let left = (w.flame_left(f, w.tick) as f32 - v.sub_tick).max(0.0);
        let frame = frame_for(1.0 - left / flame_total, 5);
        let piece = explosion_piece(f.x as i32, f.y as i32, &burning);
        v.sprites.draw(
            &format!("{piece}_{frame}"),
            f.x as f32 * TILE,
            f.y as f32 * TILE,
            TILE,
            TILE,
            WHITE,
        );
    }

    // Players, lower ones over higher ones.
    let mut players: Vec<_> = w.players.values().filter(|p| p.alive).collect();
    players.sort_by(|a, b| {
        let ay = World::visual_position(a, &v.rules, v.sub_tick).1;
        let by = World::visual_position(b, &v.rules, v.sub_tick).1;
        ay.total_cmp(&by)
    });
    for p in players {
        let (x, y) = World::visual_position(p, &v.rules, v.sub_tick);
        let (px, py) = (x * TILE, y * TILE);
        let walk = if p.moving {
            let total = v.rules.ticks_per_cell_at(p.speed).max(1) as f32;
            frame_for((p.move_progress as f32 + v.sub_tick) / total, 4)
        } else {
            0
        };
        draw_ellipse(
            px + TILE / 2.0,
            py + TILE - 5.0,
            20.0,
            7.0,
            0.0,
            Color::new(0.0, 0.0, 0.0, 0.3),
        );
        if Some(p.id) == v.my_id {
            // A ring at the feet in our own colour: find yourself at a glance.
            let pulse = 0.55 + 0.25 * (t * 5.0).sin();
            draw_ellipse_lines(
                px + TILE / 2.0,
                py + TILE - 5.0,
                25.0,
                9.0,
                0.0,
                3.0,
                palette::with_alpha(palette::player_text(p.id), pulse),
            );
        }
        v.sprites
            .draw(&player_sprite(p.id, p.dir, walk), px, py, TILE, TILE, WHITE);
        if Some(p.id) == v.my_id {
            let bob = (t * 6.0).sin() * 4.0;
            let (ax, ay) = (px + TILE / 2.0, py - 10.0 + bob);
            let c = palette::player_text(p.id);
            draw_triangle(
                vec2(ax - 11.0, ay - 12.0),
                vec2(ax + 11.0, ay - 12.0),
                vec2(ax, ay + 2.0),
                BLACK,
            );
            draw_triangle(
                vec2(ax - 8.0, ay - 10.0),
                vec2(ax + 8.0, ay - 10.0),
                vec2(ax, ay - 1.0),
                c,
            );
        }
    }

    v.fx.draw(v.sprites);
}

/// Cells about to be walled in pulse red, in closing order, so nobody is
/// surprised by the next one.
fn draw_sudden_death_warning(v: &BoardView, now_tick: f32, t: f32) {
    let rules = &v.rules;
    if rules.sudden_death_tick >= rules.round_time_ticks {
        return;
    }
    const LOOKAHEAD: f32 = 90.0;
    let start = rules.sudden_death_tick as f32;
    if now_tick < start - LOOKAHEAD {
        return;
    }
    let w = v.world;
    for (i, (x, y)) in World::closing_order(w.width, w.height)
        .into_iter()
        .enumerate()
    {
        let closes = start + (SUDDEN_DEATH_TICKS_PER_CELL as usize * i) as f32;
        let ahead = closes - now_tick;
        if ahead < 0.0 {
            continue;
        }
        if ahead > LOOKAHEAD {
            break;
        }
        if w.tile(x, y) == Tile::Solid {
            continue;
        }
        let near = 1.0 - ahead / LOOKAHEAD;
        let pulse = 0.5 + 0.5 * (t * 10.0 + i as f32 * 0.3).sin();
        draw_rectangle(
            x as f32 * TILE,
            y as f32 * TILE,
            TILE,
            TILE,
            Color::new(0.9, 0.1, 0.1, 0.12 + 0.3 * near * pulse),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells(list: &[(i32, i32)]) -> HashSet<(i32, i32)> {
        list.iter().copied().collect()
    }

    #[test]
    fn a_horizontal_blast_has_tips_pointing_outward() {
        let burning = cells(&[(1, 1), (2, 1), (3, 1)]);
        assert_eq!(explosion_piece(1, 1, &burning), "expl_tip_left");
        assert_eq!(explosion_piece(2, 1, &burning), "expl_arm_h");
        assert_eq!(explosion_piece(3, 1, &burning), "expl_tip_right");
    }

    #[test]
    fn a_crossing_is_a_centre_and_a_lone_cell_too() {
        let burning = cells(&[(2, 1), (1, 2), (2, 2), (3, 2), (2, 3), (9, 9)]);
        assert_eq!(explosion_piece(2, 2, &burning), "expl_center");
        assert_eq!(explosion_piece(2, 1, &burning), "expl_tip_up");
        assert_eq!(explosion_piece(2, 3, &burning), "expl_tip_down");
        assert_eq!(explosion_piece(9, 9, &burning), "expl_center");
    }

    #[test]
    fn progress_one_is_the_last_frame() {
        assert_eq!(frame_for(0.0, 4), 0);
        assert_eq!(frame_for(0.99, 4), 3);
        assert_eq!(frame_for(1.0, 4), 3);
    }
}
