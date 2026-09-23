//! The client's picture of a running match, rebuilt from server frames.
//!
//! A `KEYFRAME` replaces everything; a `DELTA` edits it. Nothing here predicts
//! or simulates -- the server is the only authority on what happened. The one
//! thing added on top is a list of [`FxEvent`]s: moments worth an effect
//! (an explosion, a crate breaking, a pickup) that the state alone would only
//! show as "before" and "after".

use std::collections::{BTreeMap, HashMap};

use bomber_domain::board::Tile;
use bomber_domain::game::phases::sudden_death;
use bomber_domain::game::{PowerupKind, Rules};
use bomber_domain::shared::Direction;
use bomber_protocol::{DeltaRecord, Keyframe};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerView {
    pub id: u8,
    pub alive: bool,
    pub moving: bool,
    /// Destination cell while moving: a step commits the moment it starts.
    pub x: u8,
    pub y: u8,
    pub dir: Direction,
    pub move_progress: u8,
    pub bombs_max: u8,
    pub flame: u8,
    pub speed: u8,
    pub score: u16,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BombView {
    pub id: u16,
    pub owner: u8,
    pub x: u8,
    pub y: u8,
    /// Fuse as of `seen_tick`. Deltas do not count it down, so readers age it.
    pub fuse: u16,
    pub seen_tick: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlameView {
    pub x: u8,
    pub y: u8,
    /// Remaining burn as of `seen_tick`.
    pub ticks: u8,
    pub seen_tick: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerupView {
    pub id: u16,
    pub x: u8,
    pub y: u8,
    pub kind: PowerupKind,
}

/// Something that happened this tick and deserves more than a state change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FxEvent {
    Explosion {
        x: u8,
        y: u8,
        arms: [u8; 4],
    },
    CrateBroken {
        x: u8,
        y: u8,
    },
    BombPlaced {
        x: u8,
        y: u8,
    },
    PowerupTaken {
        player: u8,
        x: u8,
        y: u8,
        kind: PowerupKind,
    },
    PowerupBurned {
        x: u8,
        y: u8,
    },
    PlayerDied {
        player: u8,
        x: u8,
        y: u8,
    },
    WallClosed {
        x: u8,
        y: u8,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct World {
    pub width: u8,
    pub height: u8,
    tiles: Vec<Tile>,
    pub players: BTreeMap<u8, PlayerView>,
    pub bombs: BTreeMap<u16, BombView>,
    pub flames: HashMap<(u8, u8), FlameView>,
    pub powerups: BTreeMap<u16, PowerupView>,
    pub ticks_remaining: u32,
    /// The server tick this state represents; resets to 0 each match.
    pub tick: u32,
}

impl World {
    pub fn from_keyframe(frame: &Keyframe) -> Self {
        let players = frame
            .players
            .iter()
            .map(|p| {
                (
                    p.id.raw(),
                    PlayerView {
                        id: p.id.raw(),
                        alive: p.alive,
                        moving: p.moving,
                        x: p.x,
                        y: p.y,
                        dir: p.dir,
                        move_progress: p.move_progress,
                        bombs_max: p.bombs_max,
                        flame: p.flame,
                        speed: p.speed,
                        score: p.score,
                    },
                )
            })
            .collect();
        let bombs = frame
            .bombs
            .iter()
            .map(|b| {
                (
                    b.id,
                    BombView {
                        id: b.id,
                        owner: b.owner.raw(),
                        x: b.x,
                        y: b.y,
                        fuse: b.fuse_remaining,
                        seen_tick: frame.tick,
                    },
                )
            })
            .collect();
        let flames = frame
            .flames
            .iter()
            .map(|f| {
                (
                    (f.x, f.y),
                    FlameView {
                        x: f.x,
                        y: f.y,
                        ticks: f.ticks_remaining,
                        seen_tick: frame.tick,
                    },
                )
            })
            .collect();
        let powerups = frame
            .powerups
            .iter()
            .map(|p| {
                (
                    p.id,
                    PowerupView {
                        id: p.id,
                        x: p.x,
                        y: p.y,
                        kind: p.kind,
                    },
                )
            })
            .collect();
        World {
            width: frame.grid.width(),
            height: frame.grid.height(),
            tiles: frame.grid.cells().to_vec(),
            players,
            bombs,
            flames,
            powerups,
            ticks_remaining: frame.ticks_remaining,
            tick: frame.tick,
        }
    }

    /// An empty board of the given tiles, for tests.
    #[cfg(test)]
    pub fn from_tiles(width: u8, height: u8, tiles: Vec<Tile>) -> Self {
        assert_eq!(tiles.len(), width as usize * height as usize);
        World {
            width,
            height,
            tiles,
            players: BTreeMap::new(),
            bombs: BTreeMap::new(),
            flames: HashMap::new(),
            powerups: BTreeMap::new(),
            ticks_remaining: 0,
            tick: 0,
        }
    }

    #[cfg(test)]
    pub fn set_tile_for_test(&mut self, x: u8, y: u8, tile: Tile) {
        self.set_tile(x, y, tile);
    }

    /// Out of bounds reads as a wall, which is what every caller wants.
    pub fn tile(&self, x: i32, y: i32) -> Tile {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Tile::Solid;
        }
        self.tiles[y as usize * self.width as usize + x as usize]
    }

    fn set_tile(&mut self, x: u8, y: u8, tile: Tile) {
        if x < self.width && y < self.height {
            let i = y as usize * self.width as usize + x as usize;
            self.tiles[i] = tile;
        }
    }

    pub fn bomb_at(&self, x: i32, y: i32) -> bool {
        self.bombs
            .values()
            .any(|b| b.x as i32 == x && b.y as i32 == y)
    }

    /// Apply one delta. The caller has checked `base_tick`.
    pub fn apply(&mut self, tick: u32, records: &[DeltaRecord], fx: &mut Vec<FxEvent>) {
        for record in records {
            match *record {
                DeltaRecord::PlayerState {
                    id,
                    alive,
                    moving,
                    x,
                    y,
                    dir,
                    move_progress,
                } => {
                    let entry = self.players.entry(id.raw()).or_insert(PlayerView {
                        id: id.raw(),
                        alive,
                        moving,
                        x,
                        y,
                        dir,
                        move_progress,
                        bombs_max: 1,
                        flame: 1,
                        speed: 0,
                        score: 0,
                    });
                    entry.alive = alive;
                    entry.moving = moving;
                    entry.x = x;
                    entry.y = y;
                    entry.dir = dir;
                    entry.move_progress = move_progress;
                }
                DeltaRecord::PlayerStats {
                    id,
                    bombs_max,
                    flame,
                    speed,
                    score,
                } => {
                    if let Some(p) = self.players.get_mut(&id.raw()) {
                        p.bombs_max = bombs_max;
                        p.flame = flame;
                        p.speed = speed;
                        p.score = score;
                    }
                }
                DeltaRecord::BombAdd {
                    id,
                    owner,
                    x,
                    y,
                    fuse,
                } => {
                    self.bombs.insert(
                        id,
                        BombView {
                            id,
                            owner: owner.raw(),
                            x,
                            y,
                            fuse,
                            seen_tick: tick,
                        },
                    );
                    fx.push(FxEvent::BombPlaced { x, y });
                }
                DeltaRecord::BombRemove { id } => {
                    self.bombs.remove(&id);
                }
                DeltaRecord::Explosion {
                    x,
                    y,
                    up,
                    down,
                    left,
                    right,
                } => fx.push(FxEvent::Explosion {
                    x,
                    y,
                    arms: [up, down, left, right],
                }),
                DeltaRecord::TileSet { x, y, tile } => {
                    if tile == Tile::Empty && self.tile(x as i32, y as i32) == Tile::Soft {
                        fx.push(FxEvent::CrateBroken { x, y });
                    }
                    self.set_tile(x, y, tile);
                }
                DeltaRecord::PowerupAdd { id, x, y, kind } => {
                    self.powerups.insert(id, PowerupView { id, x, y, kind });
                }
                DeltaRecord::PowerupRemove { id, taken_by } => {
                    if let Some(p) = self.powerups.remove(&id) {
                        fx.push(match taken_by {
                            Some(player) => FxEvent::PowerupTaken {
                                player: player.raw(),
                                x: p.x,
                                y: p.y,
                                kind: p.kind,
                            },
                            None => FxEvent::PowerupBurned { x: p.x, y: p.y },
                        });
                    }
                }
                DeltaRecord::PlayerDeath { id, .. } => {
                    if let Some(p) = self.players.get_mut(&id.raw()) {
                        p.alive = false;
                        fx.push(FxEvent::PlayerDied {
                            player: id.raw(),
                            x: p.x,
                            y: p.y,
                        });
                    }
                }
                DeltaRecord::FlameAdd { x, y, ticks } => {
                    self.flames.insert(
                        (x, y),
                        FlameView {
                            x,
                            y,
                            ticks,
                            seen_tick: tick,
                        },
                    );
                }
                DeltaRecord::FlameRemove { x, y } => {
                    self.flames.remove(&(x, y));
                }
                DeltaRecord::Timer { ticks_remaining } => self.ticks_remaining = ticks_remaining,
                DeltaRecord::WallClosed { x, y } => {
                    self.set_tile(x, y, Tile::Solid);
                    fx.push(FxEvent::WallClosed { x, y });
                }
            }
        }
        self.tick = tick;
    }

    /// A bomb's fuse aged to `now`.
    pub fn fuse_left(&self, bomb: &BombView, now: u32) -> u16 {
        bomb.fuse
            .saturating_sub(now.saturating_sub(bomb.seen_tick).min(u16::MAX as u32) as u16)
    }

    /// A flame's remaining burn aged to `now`.
    pub fn flame_left(&self, flame: &FlameView, now: u32) -> u8 {
        flame
            .ticks
            .saturating_sub(now.saturating_sub(flame.seen_tick).min(255) as u8)
    }

    /// Where a player is drawn, in cells, including the part of a step already
    /// taken. `sub_tick` (0..1) is how far wall-clock time has moved past the
    /// last state, so motion stays smooth on screens faster than 60 Hz.
    pub fn visual_position(player: &PlayerView, rules: &Rules, sub_tick: f32) -> (f32, f32) {
        let (x, y) = (player.x as f32, player.y as f32);
        if !player.moving {
            return (x, y);
        }
        let total = rules.ticks_per_cell_at(player.speed).max(1) as f32;
        let done = ((player.move_progress as f32 + sub_tick) / total).clamp(0.0, 1.0);
        let (dx, dy) = player.dir.delta();
        (x - dx as f32 * (1.0 - done), y - dy as f32 * (1.0 - done))
    }

    /// Cells in the exact order sudden death walls them in, as the server
    /// computes it. Cell `i` closes at `sudden_death_tick + 6 * i`.
    pub fn closing_order(width: u8, height: u8) -> Vec<(i32, i32)> {
        sudden_death::closing_order(width, height)
            .into_iter()
            .map(|c| (c.x as i32, c.y as i32))
            .collect()
    }
}

/// Sudden death seals one cell every this many ticks.
pub const SUDDEN_DEATH_TICKS_PER_CELL: u32 = sudden_death::SUDDEN_DEATH_PERIOD;

#[cfg(test)]
mod tests {
    use super::*;
    use bomber_domain::board::TileGrid;
    use bomber_domain::shared::PlayerId;
    use bomber_protocol::PlayerSnapshot;

    pub fn keyframe() -> Keyframe {
        let mut cells = vec![Tile::Empty; 7 * 5];
        cells[2] = Tile::Soft;
        cells[3] = Tile::Solid;
        Keyframe {
            tick: 30,
            grid: TileGrid::from_cells(7, 5, cells).unwrap(),
            players: vec![PlayerSnapshot {
                id: PlayerId::new(0),
                alive: true,
                moving: false,
                x: 1,
                y: 1,
                dir: Direction::Down,
                move_progress: 0,
                bombs_max: 1,
                flame: 2,
                speed: 0,
                score: 0,
            }],
            bombs: vec![],
            flames: vec![],
            powerups: vec![],
            ticks_remaining: 1000,
        }
    }

    #[test]
    fn a_keyframe_becomes_a_world() {
        let world = World::from_keyframe(&keyframe());
        assert_eq!((world.width, world.height), (7, 5));
        assert_eq!(world.tile(2, 0), Tile::Soft);
        assert_eq!(world.tile(3, 0), Tile::Solid);
        assert_eq!(world.tile(-1, 0), Tile::Solid, "outside is wall");
        assert_eq!(world.players[&0].flame, 2);
    }

    #[test]
    fn deltas_edit_the_world_and_report_effects() {
        let mut world = World::from_keyframe(&keyframe());
        let mut fx = Vec::new();
        world.apply(
            31,
            &[
                DeltaRecord::BombAdd {
                    id: 9,
                    owner: PlayerId::new(0),
                    x: 1,
                    y: 1,
                    fuse: 120,
                },
                DeltaRecord::TileSet {
                    x: 2,
                    y: 0,
                    tile: Tile::Empty,
                },
                DeltaRecord::FlameAdd {
                    x: 2,
                    y: 0,
                    ticks: 30,
                },
            ],
            &mut fx,
        );
        assert!(world.bomb_at(1, 1));
        assert_eq!(world.tile(2, 0), Tile::Empty);
        assert_eq!(world.tick, 31);
        assert!(fx.contains(&FxEvent::CrateBroken { x: 2, y: 0 }));
        assert!(fx.contains(&FxEvent::BombPlaced { x: 1, y: 1 }));

        let bomb = world.bombs[&9];
        assert_eq!(world.fuse_left(&bomb, 61), 90, "aged by 30 ticks");
        let flame = world.flames[&(2, 0)];
        assert_eq!(world.flame_left(&flame, 100), 0, "never below zero");
    }

    #[test]
    fn a_death_reports_where_it_happened() {
        let mut world = World::from_keyframe(&keyframe());
        let mut fx = Vec::new();
        world.apply(
            31,
            &[DeltaRecord::PlayerDeath {
                id: PlayerId::new(0),
                killer: None,
            }],
            &mut fx,
        );
        assert!(!world.players[&0].alive);
        assert_eq!(
            fx,
            vec![FxEvent::PlayerDied {
                player: 0,
                x: 1,
                y: 1
            }]
        );
    }

    /// x/y is already the destination; the drawn position trails behind it.
    #[test]
    fn a_moving_player_is_drawn_between_cells() {
        let rules = Rules::default();
        let mut p = World::from_keyframe(&keyframe()).players[&0];
        p.moving = true;
        p.x = 2;
        p.dir = Direction::Right;
        p.move_progress = rules.ticks_per_cell_at(0) / 2;
        let (x, y) = World::visual_position(&p, &rules, 0.0);
        assert!((x - 1.5).abs() < 1e-4, "{x}");
        assert_eq!(y, 1.0);
    }

    #[test]
    fn closing_order_starts_at_the_top_left_and_covers_every_interior_cell() {
        let order = World::closing_order(7, 5);
        assert_eq!(order[0], (1, 1));
        assert_eq!(order.len(), 5 * 3);
        assert_eq!(order[4], (5, 1), "clockwise along the top first");
    }
}
