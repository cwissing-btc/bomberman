//! The built-in bot: picks the next action for the local player.
//!
//! A port of the hackathon team's time-aware Python bot. It makes no game
//! decisions of its own -- it predicts danger in order to choose *which* of the
//! ordinary action codes to send. Every rule constant comes from MATCH_INIT.
//!
//! Model
//! -----
//! * Every cell gets **danger windows** `[start, end]` in ticks from now:
//!   burning flames, the predicted blast of every bomb (with its owner's
//!   *current* radius, since power-ups grow it), **chain reactions**, and the
//!   exact sudden-death closing order.
//! * A step takes `ticks_per_cell_at(speed)` ticks and binds to the target
//!   cell the moment it starts, so a cell must be safe from the moment the
//!   step begins until it can be left again.
//! * Path search runs over (cell, time) and treats **waiting** as a move. A
//!   *haven* is a cell that stays safe for a full fuse-plus-flame cycle.
//!
//! Priorities: survive, then bomb (when worth it *and* the escape is fast
//! enough), then head for a target (power-up, or a bomb spot next to crates or
//! opponents), then wait.

use std::collections::{HashMap, HashSet, VecDeque};

use bomber_domain::board::Tile;
use bomber_domain::game::{PowerupKind, Rules};
use bomber_protocol::Action;

use crate::world::{PlayerView, World, SUDDEN_DEATH_TICKS_PER_CELL};

type C = (i32, i32);

const DIRS: [C; 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
const INF: i64 = 1 << 30;
/// Planning depth in steps.
const K_MAX: usize = 20;
/// Bonus for the target pursued last time, so the bot does not flip-flop.
const HYSTERESIS: f64 = 1.25;
/// After a bomb command, wait this long for BOMB_ADD instead of sending another.
pub const BOMB_COOLDOWN_TICKS: u32 = 12;
/// After sending a move, do not bomb for this many ticks.
pub const MOVE_SETTLE_TICKS: u32 = 2;
/// How long before sudden death the retreat from the edge begins.
const SD_PREP_TICKS: i64 = 20 * 60;
/// A "deep" haven is walled in no sooner than this.
const SD_DEEP_TICKS: i64 = 8 * 60;
/// Latency and jitter: assume danger a little early and a little long.
pub const SAFETY_MARGIN_TICKS: i64 = 6;
const TICKS_PER_SECOND: f64 = 60.0;

fn step_action(d: C) -> Action {
    match d {
        (0, -1) => Action::Up,
        (0, 1) => Action::Down,
        (-1, 0) => Action::Left,
        _ => Action::Right,
    }
}

fn step_bomb_action(d: C) -> Action {
    match d {
        (0, -1) => Action::UpBomb,
        (0, 1) => Action::DownBomb,
        (-1, 0) => Action::LeftBomb,
        _ => Action::RightBomb,
    }
}

fn is_step(a: Action) -> bool {
    matches!(a, Action::Up | Action::Down | Action::Left | Action::Right)
}

/// A bomb is a one-shot: on a frozen state, repeat only the movement part.
pub fn without_bomb(a: Action) -> Option<Action> {
    Some(match a {
        Action::Bomb => Action::Idle,
        Action::UpBomb => Action::Up,
        Action::DownBomb => Action::Down,
        Action::LeftBomb => Action::Left,
        Action::RightBomb => Action::Right,
        _ => return None,
    })
}

fn powerup_value(kind: PowerupKind) -> f64 {
    match kind {
        PowerupKind::Flame => 3.0,
        PowerupKind::ExtraBomb => 2.5,
        PowerupKind::Speed => 2.0,
    }
}

// -- geometry ----------------------------------------------------------------

fn free(state: &World, c: C) -> bool {
    state.tile(c.0, c.1) == Tile::Empty
}

/// Free, in bounds, and no bomb on it.
pub fn walkable(state: &World, x: i32, y: i32) -> bool {
    free(state, (x, y)) && !state.bomb_at(x, y)
}

/// Cells a bomb at (bx, by) with `radius` reaches: a cross, stopped by walls;
/// a crate is hit and then stops the arm.
pub fn blast_cells(state: &World, bx: i32, by: i32, radius: u8) -> HashSet<C> {
    let mut cells = HashSet::from([(bx, by)]);
    for (dx, dy) in DIRS {
        for r in 1..=radius.max(1) as i32 {
            let (x, y) = (bx + dx * r, by + dy * r);
            if x < 0 || y < 0 || x >= state.width as i32 || y >= state.height as i32 {
                break;
            }
            match state.tile(x, y) {
                Tile::Solid => break,
                Tile::Soft => {
                    cells.insert((x, y));
                    break;
                }
                Tile::Empty => {
                    cells.insert((x, y));
                }
            }
        }
    }
    cells
}

// -- danger model --------------------------------------------------------------

/// The current match tick. The frame tick *is* the match tick; the timer is
/// only refreshed by keyframes and can be up to 29 ticks stale.
fn match_tick(state: &World, rules: &Rules, now_tick: u32) -> Option<i64> {
    if now_tick != 0 {
        return Some(now_tick as i64);
    }
    if rules.round_time_ticks == 0 || state.ticks_remaining == 0 {
        return None;
    }
    Some(rules.round_time_ticks as i64 - state.ticks_remaining as i64)
}

fn radius_of(state: &World, owner: u8, rules: &Rules) -> u8 {
    // Unknown owner: assume the rule maximum, power-ups included.
    state
        .players
        .get(&owner)
        .map_or(rules.max_flame, |p| p.flame)
}

/// A counter aged by the ticks since it was stamped. Keyframes come every 30
/// ticks and deltas do not count fuses down.
fn remaining(value: i64, seen_tick: u32, now_tick: u32) -> i64 {
    (value - (now_tick as i64 - seen_tick as i64).max(0)).max(0)
}

type Intervals = HashMap<C, Vec<(i64, i64)>>;

fn add(iv: &mut Intervals, cell: C, s: i64, e: i64) {
    iv.entry(cell).or_default().push((s.max(0), e));
}

/// Cell -> windows (start, end) in ticks from now in which it is lethal.
/// `extra_bomb` simulates one of our own, placed right now.
pub fn danger_intervals(
    state: &World,
    rules: &Rules,
    extra_bomb: Option<(C, u8)>,
    now_tick: u32,
) -> Intervals {
    let mut iv = Intervals::new();
    let margin = SAFETY_MARGIN_TICKS;

    for f in state.flames.values() {
        add(
            &mut iv,
            (f.x as i32, f.y as i32),
            0,
            remaining(f.ticks as i64, f.seen_tick, now_tick) + margin,
        );
    }

    // Bombs including chains: det = min(own fuse, fuse of any bomb whose
    // blast reaches this one), iterated to a fixpoint.
    let mut specs: Vec<(C, i64, HashSet<C>)> = state
        .bombs
        .values()
        .map(|b| {
            (
                (b.x as i32, b.y as i32),
                remaining(b.fuse as i64, b.seen_tick, now_tick),
                blast_cells(
                    state,
                    b.x as i32,
                    b.y as i32,
                    radius_of(state, b.owner, rules),
                ),
            )
        })
        .collect();
    if let Some((cell, radius)) = extra_bomb {
        specs.push((
            cell,
            rules.bomb_fuse_ticks as i64,
            blast_cells(state, cell.0, cell.1, radius),
        ));
    }
    let mut changed = true;
    while changed {
        changed = false;
        for a in 0..specs.len() {
            for b in 0..specs.len() {
                if a != b && specs[a].2.contains(&specs[b].0) && specs[a].1 < specs[b].1 {
                    specs[b].1 = specs[a].1;
                    changed = true;
                }
            }
        }
    }
    for (_, det, blast) in &specs {
        for c in blast {
            add(
                &mut iv,
                *c,
                det - margin,
                det + rules.flame_duration_ticks as i64 + margin,
            );
        }
    }

    sudden_death_intervals(state, rules, &mut iv, now_tick);
    iv
}

/// Every interior cell is lethal for good from its exact closing tick.
fn sudden_death_intervals(state: &World, rules: &Rules, iv: &mut Intervals, now_tick: u32) {
    let total = rules.round_time_ticks;
    let Some(now) = match_tick(state, rules, now_tick) else {
        return;
    };
    if total == 0 || rules.sudden_death_tick >= total {
        return;
    }
    let horizon = rules.bomb_fuse_ticks as i64 * 4;
    for (i, cell) in World::closing_order(state.width, state.height)
        .into_iter()
        .enumerate()
    {
        let start =
            rules.sudden_death_tick as i64 + SUDDEN_DEATH_TICKS_PER_CELL as i64 * i as i64 - now;
        if start > horizon {
            break;
        }
        add(iv, cell, start, INF);
    }
}

/// Bombs a player could still place right now.
pub fn bomb_capacity(state: &World, player: u8) -> u8 {
    let Some(p) = state.players.get(&player) else {
        return 0;
    };
    let active = state.bombs.values().filter(|b| b.owner == player).count() as u8;
    p.bombs_max.max(1).saturating_sub(active)
}

/// Cells a living opponent with a spare bomb could hit *now*. Not certain
/// danger, but no place to linger.
pub fn threat_cells(state: &World, my_id: u8) -> HashSet<C> {
    let mut cells = HashSet::new();
    for p in state.players.values() {
        if p.id == my_id || !p.alive || bomb_capacity(state, p.id) == 0 {
            continue;
        }
        cells.extend(blast_cells(state, p.x as i32, p.y as i32, p.flame));
    }
    cells
}

/// At most one walkable neighbour: few escape routes, a worthwhile target.
fn is_cornered(state: &World, x: i32, y: i32) -> bool {
    DIRS.iter()
        .filter(|(dx, dy)| walkable(state, x + dx, y + dy))
        .count()
        <= 1
}

/// The bot's view of one state: passability, danger windows, threat zones.
pub struct Sight<'a> {
    state: &'a World,
    blocked: HashSet<C>,
    intervals: Intervals,
    threat: HashSet<C>,
    closing: HashMap<C, i64>,
    pub sd_prep: bool,
    haven_ticks: i64,
}

impl<'a> Sight<'a> {
    pub fn new(
        state: &'a World,
        rules: &Rules,
        my_id: u8,
        extra_bomb: Option<(C, u8)>,
        now_tick: u32,
    ) -> Sight<'a> {
        let mut blocked: HashSet<C> = state
            .bombs
            .values()
            .map(|b| (b.x as i32, b.y as i32))
            .collect();
        if let Some((cell, _)) = extra_bomb {
            blocked.insert(cell);
        }
        for p in state.players.values() {
            if p.id != my_id && p.alive {
                blocked.insert((p.x as i32, p.y as i32));
            }
        }
        let mut closing = HashMap::new();
        let mut sd_prep = false;
        if let Some(now) = match_tick(state, rules, now_tick) {
            if rules.sudden_death_tick < rules.round_time_ticks {
                for (i, cell) in World::closing_order(state.width, state.height)
                    .into_iter()
                    .enumerate()
                {
                    closing.insert(
                        cell,
                        rules.sudden_death_tick as i64
                            + SUDDEN_DEATH_TICKS_PER_CELL as i64 * i as i64
                            - now,
                    );
                }
                sd_prep = rules.sudden_death_tick as i64 - now <= SD_PREP_TICKS;
            }
        }
        Sight {
            state,
            blocked,
            intervals: danger_intervals(state, rules, extra_bomb, now_tick),
            threat: threat_cells(state, my_id),
            closing,
            sd_prep,
            haven_ticks: rules.bomb_fuse_ticks as i64 + rules.flame_duration_ticks as i64 + 10,
        }
    }

    fn passable(&self, c: C) -> bool {
        free(self.state, c) && !self.blocked.contains(&c)
    }

    fn safe(&self, c: C, t0: i64, t1: i64) -> bool {
        self.intervals
            .get(&c)
            .is_none_or(|w| w.iter().all(|&(s, e)| !(s <= t1 && e >= t0)))
    }

    fn haven(&self, c: C, t: i64) -> bool {
        self.safe(c, t, t + self.haven_ticks)
    }

    /// A haven outside every enemy bomb line.
    fn quiet_haven(&self, c: C, t: i64) -> bool {
        !self.threat.contains(&c) && self.haven(c, t)
    }

    /// Ticks until sudden death walls the cell in (INF: never, or unknown).
    fn time_to_close(&self, c: C) -> i64 {
        self.closing.get(&c).copied().unwrap_or(INF)
    }

    /// A haven the closing border will leave alone for a good while yet.
    fn deep_haven(&self, c: C, t: i64) -> bool {
        self.haven(c, t) && self.time_to_close(c) - t >= SD_DEEP_TICKS
    }
}

// -- time-aware search -----------------------------------------------------------

type Node = (C, usize);

fn extract_path(parent: &HashMap<Node, Option<Node>>, mut node: Node) -> Vec<C> {
    let mut path = Vec::new();
    while let Some(Some(prev)) = parent.get(&node) {
        path.push(node.0);
        node = *prev;
    }
    path.reverse();
    path
}

/// Breadth-first over (cell, step). Waiting is a move. Returns the path and
/// whether `goal` was reached; without a hit, the path to the state that
/// survives longest.
fn search(
    world: &Sight,
    start: C,
    t_free: i64,
    step: i64,
    goal: impl Fn(C, i64) -> bool,
) -> (Vec<C>, bool) {
    let start_node = (start, 0);
    let mut parent: HashMap<Node, Option<Node>> = HashMap::from([(start_node, None)]);
    let mut queue = VecDeque::from([start_node]);
    let mut deepest = start_node;
    while let Some((cell, k)) = queue.pop_front() {
        let t = t_free + k as i64 * step;
        if goal(cell, t) {
            return (extract_path(&parent, (cell, k)), true);
        }
        if k > deepest.1 {
            deepest = (cell, k);
        }
        if k >= K_MAX {
            continue;
        }
        let t2 = t + step;
        // Walk first, then wait: whoever reaches a state first shapes the
        // path, and leaving early keeps more time in hand than waiting.
        for (dx, dy) in DIRS {
            let n = (cell.0 + dx, cell.1 + dy);
            let next = (n, k + 1);
            if parent.contains_key(&next) || !world.passable(n) || !world.safe(n, t, t2) {
                continue;
            }
            parent.insert(next, Some((cell, k)));
            queue.push_back(next);
        }
        let next = (cell, k + 1);
        if !parent.contains_key(&next) && world.safe(cell, t, t2) {
            parent.insert(next, Some((cell, k)));
            queue.push_back(next);
        }
    }
    (extract_path(&parent, deepest), false)
}

/// Nearest haven -- preferably out of enemy bomb lines if that costs at most
/// two extra steps, and deeper inside once sudden death is near.
fn escape_path(world: &Sight, start: C, t_free: i64, step: i64) -> (Vec<C>, bool) {
    let (path, found) = search(world, start, t_free, step, |c, t| world.haven(c, t));
    if found && world.sd_prep {
        let (deep, ok) = search(world, start, t_free, step, |c, t| world.deep_haven(c, t));
        if ok && deep.len() <= path.len() + 3 {
            return (deep, true);
        }
    }
    if found && !world.threat.is_empty() {
        let (quiet, ok) = search(world, start, t_free, step, |c, t| world.quiet_haven(c, t));
        if ok && quiet.len() <= path.len() + 2 {
            return (quiet, true);
        }
    }
    (path, found)
}

/// Sudden-death fallback: the safely reachable cell that closes last.
fn latest_closing_path(world: &Sight, start: C, t_free: i64, step: i64) -> Vec<C> {
    let start_node = (start, 0);
    let mut parent: HashMap<Node, Option<Node>> = HashMap::from([(start_node, None)]);
    let mut queue = VecDeque::from([start_node]);
    let mut best = (world.time_to_close(start) - t_free, start_node);
    while let Some((cell, k)) = queue.pop_front() {
        let t = t_free + k as i64 * step;
        let margin = world.time_to_close(cell) - t;
        if margin > best.0 && world.safe(cell, t, t + step) {
            best = (margin, (cell, k));
        }
        if k >= K_MAX {
            continue;
        }
        for (dx, dy) in DIRS {
            let n = (cell.0 + dx, cell.1 + dy);
            let next = (n, k + 1);
            if parent.contains_key(&next) || !world.passable(n) || !world.safe(n, t, t + step) {
                continue;
            }
            parent.insert(next, Some((cell, k)));
            queue.push_back(next);
        }
    }
    if best.1 == start_node {
        Vec::new()
    } else {
        extract_path(&parent, best.1)
    }
}

fn first_action(start: C, path: &[C]) -> Action {
    match path.first() {
        None => Action::Idle,
        Some(&c) if c == start => Action::Idle,
        Some(&c) => step_action((c.0 - start.0, c.1 - start.1)),
    }
}

// -- the bot ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SavedBomb {
    action: Action,
    path: Vec<C>,
    cell: C,
    step: i64,
    t_free: i64,
}

/// Memory between ticks. Reset per match: every tick in here is a match tick.
#[derive(Debug, Clone)]
struct Memo {
    tick: Option<u32>,
    plan: Vec<C>,
    plan_start: Option<C>,
    plan_t0: f64,
    plan_step_s: f64,
    plan_delay_s: f64,
    target: Option<C>,
    banned: HashMap<C, u32>,
    bomb_sent_tick: Option<u32>,
    last_move_tick: Option<u32>,
    match_id: Option<u32>,
    bomb_action: Option<SavedBomb>,
}

impl Default for Memo {
    fn default() -> Self {
        Memo {
            tick: None,
            plan: Vec::new(),
            plan_start: None,
            plan_t0: 0.0,
            plan_step_s: 8.0 / TICKS_PER_SECOND,
            plan_delay_s: 0.0,
            target: None,
            banned: HashMap::new(),
            bomb_sent_tick: None,
            last_move_tick: None,
            match_id: None,
            bomb_action: None,
        }
    }
}

#[derive(Debug, Default)]
pub struct Bot {
    memo: Memo,
}

/// What the bot needs to know about the match beyond the world state.
pub struct Situation<'a> {
    pub world: &'a World,
    pub rules: Rules,
    pub my_id: u8,
    /// The tick of the state held, `None` if unknown.
    pub at_tick: Option<u32>,
    pub match_id: Option<u32>,
    /// Wall clock in seconds, for dead reckoning on a frozen state.
    pub now: f64,
}

struct Decision {
    action: Action,
    path: Vec<C>,
    start: C,
    step: i64,
    t_free: i64,
}

impl Bot {
    pub fn reset(&mut self) {
        self.memo = Memo::default();
    }

    /// The next action for our own player.
    pub fn decide(&mut self, s: &Situation) -> Action {
        let Some(me) = s.world.players.get(&s.my_id).copied() else {
            return Action::Idle;
        };
        if !me.alive {
            return Action::Idle;
        }
        self.forget_previous_match(s);

        if s.at_tick.is_some() && s.at_tick == self.memo.tick {
            // Frozen state (a lost delta): keep following the plan.
            let action = self.replay(s.now);
            if is_step(action) {
                self.memo.last_move_tick = s.at_tick;
            }
            return action;
        }

        let resend = self.lost_bomb_action(s.world, &me, s.at_tick);
        let was_resend = resend.is_some();
        let d = match resend {
            Some(d) => d,
            None => self.decide_fresh(s.world, &s.rules, &me, s.at_tick),
        };
        self.memo.tick = s.at_tick;
        self.memo.plan = d.path.clone();
        self.memo.plan_start = Some(d.start);
        self.memo.plan_t0 = s.now;
        self.memo.plan_step_s = d.step as f64 / TICKS_PER_SECOND;
        self.memo.plan_delay_s = d.t_free as f64 / TICKS_PER_SECOND;

        if without_bomb(d.action).is_some() {
            if let Some(tick) = s.at_tick {
                self.memo.bomb_sent_tick = Some(tick);
                if !was_resend {
                    // Only the first command may be repeated, once.
                    self.memo.bomb_action = Some(SavedBomb {
                        action: d.action,
                        path: d.path.clone(),
                        cell: (me.x as i32, me.y as i32),
                        step: d.step,
                        t_free: d.t_free,
                    });
                }
            }
        }
        if is_step(d.action) && s.at_tick.is_some() {
            self.memo.last_move_tick = s.at_tick;
        }
        d.action
    }

    /// Repeat last tick's bomb command once if it evidently never arrived.
    ///
    /// The server takes only the newest packet per tick window, so a bomb and
    /// a following packet in the same window lose the bomb. Repeating is
    /// harmless: if the first one landed, either the step is under way
    /// (actions mid-step are ignored) or the bomb is already there.
    fn lost_bomb_action(
        &mut self,
        state: &World,
        me: &PlayerView,
        at_tick: Option<u32>,
    ) -> Option<Decision> {
        let at = at_tick?;
        let saved = self.memo.bomb_action.clone()?;
        if self.memo.bomb_sent_tick != Some(at.wrapping_sub(1)) {
            return None;
        }
        if me.moving || (me.x as i32, me.y as i32) != saved.cell {
            return None;
        }
        if state.bomb_at(saved.cell.0, saved.cell.1) {
            return None;
        }
        self.memo.bomb_action = None;
        Some(Decision {
            action: saved.action,
            path: saved.path,
            start: saved.cell,
            step: saved.step,
            t_free: saved.t_free,
        })
    }

    /// A new match (other id, or the tick went backwards) clears the memory.
    /// Ticks from the last match would otherwise keep cooldowns running for
    /// minutes.
    fn forget_previous_match(&mut self, s: &Situation) {
        let new_match = s.match_id != self.memo.match_id;
        let went_back =
            matches!((s.at_tick, self.memo.tick), (Some(now), Some(before)) if now < before);
        if new_match || went_back {
            self.reset();
            self.memo.match_id = s.match_id;
        }
    }

    /// Dead reckoning: without a new state, follow the last (time-checked)
    /// plan by wall clock, one step per `plan_step_s`. Never re-bombs.
    fn replay(&self, now: f64) -> Action {
        let plan = &self.memo.plan;
        if plan.is_empty() {
            return Action::Idle;
        }
        // A running step delays the start; one tick of lag so the replay never
        // runs ahead of the server.
        let elapsed = now - self.memo.plan_t0 - self.memo.plan_delay_s - 1.0 / TICKS_PER_SECOND;
        let elapsed = elapsed.max(0.0);
        let idx = (elapsed / self.memo.plan_step_s) as usize;
        if idx >= plan.len() {
            return Action::Idle;
        }
        // Last step: send the direction only in the first half of its slot. A
        // late direction would be taken again after the step and overshoot.
        if idx == plan.len() - 1 {
            let frac = (elapsed - idx as f64 * self.memo.plan_step_s) / self.memo.plan_step_s;
            if frac > 0.5 {
                return Action::Idle;
            }
        }
        let prev = if idx == 0 {
            self.memo.plan_start.unwrap_or(plan[0])
        } else {
            plan[idx - 1]
        };
        let next = plan[idx];
        if next == prev {
            return Action::Idle;
        }
        step_action((next.0 - prev.0, next.1 - prev.1))
    }

    fn bomb_unavailable(&self, state: &World, me: &PlayerView, now_tick: u32) -> bool {
        if bomb_capacity(state, me.id) == 0 {
            return true;
        }
        self.memo
            .bomb_sent_tick
            .is_some_and(|sent| (now_tick as i64 - sent as i64) < BOMB_COOLDOWN_TICKS as i64)
    }

    /// Crates or enemies in our cross, minus power-ups the blast would burn.
    fn bomb_worth_here(state: &World, me: &PlayerView, enemies: &[PlayerView]) -> bool {
        let blast = blast_cells(state, me.x as i32, me.y as i32, me.flame);
        let crates = blast
            .iter()
            .filter(|c| state.tile(c.0, c.1) == Tile::Soft)
            .count() as i32;
        let hits = enemies
            .iter()
            .filter(|e| blast.contains(&(e.x as i32, e.y as i32)))
            .count() as i32;
        let burned = state
            .powerups
            .values()
            .filter(|p| blast.contains(&(p.x as i32, p.y as i32)))
            .count() as i32;
        crates + 3 * hits - burned >= 1
    }

    /// Bomb and step away, if it pays and the escape is fast enough.
    /// `(Idle, [])` means: good spot, but a move was just sent -- settle first,
    /// or the bomb could land on the target of a step the server still takes.
    fn plan_attack(
        &self,
        state: &World,
        rules: &Rules,
        me: &PlayerView,
        enemies: &[PlayerView],
        step: i64,
        now_tick: u32,
    ) -> Option<(Action, Vec<C>)> {
        if self.bomb_unavailable(state, me, now_tick) || !Self::bomb_worth_here(state, me, enemies)
        {
            return None;
        }
        let cur = (me.x as i32, me.y as i32);
        let world = Sight::new(state, rules, me.id, Some((cur, me.flame)), now_tick);
        let mut best: Option<(usize, C, Vec<C>)> = None;
        for d in DIRS {
            let n = (cur.0 + d.0, cur.1 + d.1);
            if !world.passable(n) || !world.safe(n, 0, step) {
                continue;
            }
            let (path, found) = escape_path(&world, n, 0, step);
            if found && best.as_ref().is_none_or(|b| path.len() < b.0) {
                let mut full = vec![n];
                full.extend(path.iter().copied());
                best = Some((path.len(), d, full));
            }
        }
        let (_, dir, path) = best?;
        if self
            .memo
            .last_move_tick
            .is_some_and(|m| (now_tick as i64 - m as i64) < MOVE_SETTLE_TICKS as i64)
        {
            return Some((Action::Idle, Vec::new()));
        }
        Some((step_bomb_action(dir), path))
    }

    /// Value per cell: a power-up there, or a good bomb spot.
    fn target_values(
        &self,
        world: &Sight,
        me: &PlayerView,
        enemies: &[PlayerView],
    ) -> HashMap<C, f64> {
        let state = world.state;
        let mut vals: HashMap<C, f64> = HashMap::new();
        for p in state.powerups.values() {
            let c = (p.x as i32, p.y as i32);
            if world.passable(c) {
                let v = vals.entry(c).or_insert(0.0);
                *v = v.max(powerup_value(p.kind));
            }
        }
        let enemy_cells: Vec<C> = enemies.iter().map(|e| (e.x as i32, e.y as i32)).collect();
        for y in 0..state.height as i32 {
            for x in 0..state.width as i32 {
                let c = (x, y);
                if self.memo.banned.contains_key(&c) || !world.passable(c) {
                    continue;
                }
                let blast = blast_cells(state, x, y, me.flame);
                let crates = blast
                    .iter()
                    .filter(|b| state.tile(b.0, b.1) == Tile::Soft)
                    .count();
                let hit: Vec<&C> = enemy_cells.iter().filter(|e| blast.contains(e)).collect();
                let mut v = 0.0;
                if crates > 0 {
                    v = 1.0 + 0.5 * (crates as f64 - 1.0);
                }
                if !hit.is_empty() {
                    v = f64::max(v, 2.0 + 0.5 * hit.len() as f64);
                    if hit.iter().any(|e| is_cornered(state, e.0, e.1)) {
                        v += 1.5; // an opponent with no way out: strike now
                    }
                }
                if v > 0.0 {
                    if world.threat.contains(&c) {
                        v *= 0.6;
                    }
                    if world.sd_prep {
                        // Cells that will be walled in soon lose value.
                        v *=
                            (world.time_to_close(c) as f64 / SD_PREP_TICKS as f64).clamp(0.15, 1.0);
                    }
                    let e = vals.entry(c).or_insert(0.0);
                    *e = e.max(v);
                }
            }
        }
        vals
    }

    /// Path to the best target: value / (steps + 1), with hysteresis for the
    /// last target. Empty when nothing is safely reachable.
    fn plan_target(
        &mut self,
        world: &Sight,
        me: &PlayerView,
        step: i64,
        t_free: i64,
        enemies: &[PlayerView],
    ) -> Vec<C> {
        let vals = self.target_values(world, me, enemies);
        if vals.is_empty() {
            return Vec::new();
        }
        let start = (me.x as i32, me.y as i32);
        let vmax = vals.values().cloned().fold(0.0, f64::max) * HYSTERESIS;
        let mut best_util = 0.0;
        let mut best: Option<(Vec<C>, C)> = None;

        let start_node = (start, 0);
        let mut parent: HashMap<Node, Option<Node>> = HashMap::from([(start_node, None)]);
        let mut queue = VecDeque::from([start_node]);
        while let Some((cell, k)) = queue.pop_front() {
            let t = t_free + k as i64 * step;
            if vmax / (k as f64 + 1.0) <= best_util {
                break; // no improvement possible any more
            }
            if let Some(&v) = vals.get(&cell) {
                if (cell != start || t_free > 0) && world.haven(cell, t) {
                    let v = if Some(cell) == self.memo.target {
                        v * HYSTERESIS
                    } else {
                        v
                    };
                    let util = v / (k as f64 + 1.0);
                    if util > best_util {
                        let mut path = extract_path(&parent, (cell, k));
                        if path.is_empty() {
                            path = vec![cell];
                        }
                        best_util = util;
                        best = Some((path, cell));
                    }
                }
            }
            if k >= K_MAX {
                continue;
            }
            let t2 = t + step;
            // Towards a target only through havens: a cell that burns in 100
            // ticks would be fine to cross, but rule 1 then drives the bot
            // straight back to a haven, and it oscillates.
            for (dx, dy) in DIRS {
                let n = (cell.0 + dx, cell.1 + dy);
                let next = (n, k + 1);
                if parent.contains_key(&next) || !world.passable(n) || !world.haven(n, t) {
                    continue;
                }
                parent.insert(next, Some((cell, k)));
                queue.push_back(next);
            }
            let next = (cell, k + 1);
            if !parent.contains_key(&next) && world.safe(cell, t, t2) {
                parent.insert(next, Some((cell, k)));
                queue.push_back(next);
            }
        }

        match best {
            None => Vec::new(),
            Some((path, cell)) => {
                self.memo.target = Some(cell);
                path
            }
        }
    }

    fn decide_fresh(
        &mut self,
        state: &World,
        rules: &Rules,
        me: &PlayerView,
        at_tick: Option<u32>,
    ) -> Decision {
        let step = rules.ticks_per_cell_at(me.speed) as i64;
        let t_free = if me.moving {
            (step - me.move_progress as i64).max(0)
        } else {
            0
        };
        let enemies: Vec<PlayerView> = state
            .players
            .values()
            .filter(|p| p.id != me.id && p.alive)
            .copied()
            .collect();
        let now_tick = at_tick.unwrap_or(0);
        let world = Sight::new(state, rules, me.id, None, now_tick);
        let cur = (me.x as i32, me.y as i32);
        let done = |action, path| Decision {
            action,
            path,
            start: cur,
            step,
            t_free,
        };

        if let Some(at) = at_tick {
            self.memo.banned.retain(|_, until| *until > at);
        }

        // 1) Survive: if this cell is no haven, head for the nearest one.
        if !world.haven(cur, t_free) {
            let (path, found) = escape_path(&world, cur, t_free, step);
            if found || !path.is_empty() {
                return done(first_action(cur, &path), path);
            }
            return done(Action::Idle, Vec::new()); // boxed in; nothing helps
        }

        // 2) Attack: bomb and step away (only while standing).
        if t_free == 0 {
            if let Some((action, path)) =
                self.plan_attack(state, rules, me, &enemies, step, now_tick)
            {
                return done(action, path);
            }
            if Some(cur) == self.memo.target {
                if let Some(at) = at_tick {
                    if self.bomb_unavailable(state, me, now_tick)
                        && Self::bomb_worth_here(state, me, &enemies)
                        && !world.threat.contains(&cur)
                    {
                        // Good spot, just no bomb free: stay until ours has
                        // gone off instead of wandering between spots.
                        return done(Action::Idle, Vec::new());
                    }
                    // On a chosen bomb spot but no safe bomb here: ban it for
                    // a while, or the bot would be stuck on it.
                    self.memo
                        .banned
                        .insert(cur, at + rules.bomb_fuse_ticks as u32);
                    self.memo.target = None;
                }
            }
        } else if Some(cur) == self.memo.target {
            // Mid-step towards the target (x/y already is the target): finish
            // the step instead of replanning away from it.
            return done(Action::Idle, Vec::new());
        }

        // 3) Head for a target.
        let mut path = self.plan_target(&world, me, step, t_free, &enemies);
        if path.is_empty() && world.sd_prep && world.time_to_close(cur) < SD_DEEP_TICKS {
            // No target and this cell closes soon: move inward in time.
            let (deep, found) = search(&world, cur, t_free, step, |c, t| world.deep_haven(c, t));
            path = if found {
                deep
            } else {
                latest_closing_path(&world, cur, t_free, step)
            };
        }
        if path.is_empty() && world.threat.contains(&cur) {
            // No target, but standing in an enemy's line: step out of it.
            let (quiet, found) = search(&world, cur, t_free, step, |c, t| world.quiet_haven(c, t));
            path = if found { quiet } else { Vec::new() };
        }
        done(first_action(cur, &path), path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{BombView, FlameView, PowerupView};
    use bomber_domain::shared::Direction;

    const MOVES: [Action; 4] = [Action::Up, Action::Down, Action::Left, Action::Right];

    fn is_bomb(a: Action) -> bool {
        without_bomb(a).is_some()
    }

    fn is_combo(a: Action) -> bool {
        is_bomb(a) && a != Action::Bomb
    }

    /// Free field with a wall all around.
    fn open_field(w: u8, h: u8) -> World {
        let mut tiles = vec![Tile::Empty; w as usize * h as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                if x == 0 || y == 0 || x == w as usize - 1 || y == h as usize - 1 {
                    tiles[y * w as usize + x] = Tile::Solid;
                }
            }
        }
        World::from_tiles(w, h, tiles)
    }

    fn player(id: u8, x: u8, y: u8, flame: u8) -> PlayerView {
        PlayerView {
            id,
            alive: true,
            moving: false,
            x,
            y,
            dir: Direction::Down,
            move_progress: 0,
            bombs_max: 1,
            flame,
            speed: 0,
            score: 0,
        }
    }

    fn bomb(id: u16, owner: u8, x: u8, y: u8, fuse: u16, seen: u32) -> BombView {
        BombView {
            id,
            owner,
            x,
            y,
            fuse,
            seen_tick: seen,
        }
    }

    fn situation(
        world: &World,
        rules: Rules,
        at_tick: Option<u32>,
        match_id: Option<u32>,
        now: f64,
    ) -> Situation<'_> {
        Situation {
            world,
            rules,
            my_id: 0,
            at_tick,
            match_id,
            now,
        }
    }

    fn decide(bot: &mut Bot, world: &World) -> Action {
        bot.decide(&situation(world, Rules::default(), None, None, 0.0))
    }

    #[test]
    fn flees_from_flame() {
        let mut w = open_field(7, 3);
        w.players.insert(0, player(0, 3, 1, 2));
        w.flames.insert(
            (3, 1),
            FlameView {
                x: 3,
                y: 1,
                ticks: 10,
                seen_tick: 0,
            },
        );
        assert!(MOVES.contains(&decide(&mut Bot::default(), &w)));
    }

    #[test]
    fn places_a_combo_bomb_next_to_a_crate_with_an_escape() {
        let mut w = open_field(7, 5);
        w.set_tile_for_test(4, 2, Tile::Soft);
        w.players.insert(0, player(0, 3, 2, 2));
        assert!(is_combo(decide(&mut Bot::default(), &w)));
    }

    #[test]
    fn waits_when_no_move_is_safe() {
        let mut w = open_field(3, 3);
        w.players.insert(0, player(0, 1, 1, 2));
        w.flames.insert(
            (1, 1),
            FlameView {
                x: 1,
                y: 1,
                ticks: 10,
                seen_tick: 0,
            },
        );
        assert_eq!(decide(&mut Bot::default(), &w), Action::Idle);
    }

    #[test]
    fn walks_towards_a_distant_crate() {
        let mut w = open_field(9, 3);
        w.set_tile_for_test(6, 1, Tile::Soft);
        w.players.insert(0, player(0, 2, 1, 2));
        assert_eq!(decide(&mut Bot::default(), &w), Action::Right);
    }

    #[test]
    fn a_dead_player_does_nothing() {
        let mut w = open_field(5, 3);
        let mut me = player(0, 2, 1, 2);
        me.alive = false;
        w.players.insert(0, me);
        assert_eq!(decide(&mut Bot::default(), &w), Action::Idle);
    }

    #[test]
    fn no_bomb_without_an_escape() {
        let mut w = open_field(4, 3);
        w.set_tile_for_test(2, 1, Tile::Soft);
        w.players.insert(0, player(0, 1, 1, 3));
        assert!(!is_bomb(decide(&mut Bot::default(), &w)));
    }

    #[test]
    fn a_chain_reaction_shortens_the_fuse() {
        let mut w = open_field(9, 3);
        w.players.insert(1, player(1, 1, 1, 2));
        w.bombs.insert(1, bomb(1, 1, 1, 1, 2, 0));
        w.bombs.insert(2, bomb(2, 1, 3, 1, 100, 0));
        let iv = danger_intervals(&w, &Rules::default(), None, 0);
        let starts: Vec<i64> = iv[&(5, 1)].iter().map(|w| w.0).collect();
        assert_eq!(
            *starts.iter().min().unwrap(),
            (2 - SAFETY_MARGIN_TICKS).max(0)
        );
        assert!(starts.iter().all(|s| *s < 90));
    }

    #[test]
    fn a_stale_fuse_is_aged() {
        let mut w = open_field(9, 3);
        w.players.insert(1, player(1, 1, 1, 2));
        w.bombs.insert(1, bomb(1, 1, 1, 1, 100, 0));
        let fresh = danger_intervals(&w, &Rules::default(), None, 0);
        let aged = danger_intervals(&w, &Rules::default(), None, 95);
        assert_eq!(
            fresh[&(2, 1)].iter().map(|w| w.0).min(),
            Some(100 - SAFETY_MARGIN_TICKS)
        );
        assert_eq!(aged[&(2, 1)].iter().map(|w| w.0).min(), Some(0));
    }

    #[test]
    fn uses_the_owners_current_radius() {
        let mut w = open_field(9, 3);
        w.players.insert(0, player(0, 5, 1, 2));
        w.players.insert(1, player(1, 1, 1, 5));
        w.bombs.insert(1, bomb(1, 1, 1, 1, 50, 0));
        assert_eq!(decide(&mut Bot::default(), &w), Action::Right);
    }

    #[test]
    fn an_unknown_owner_is_assumed_at_max_flame() {
        let mut w = open_field(9, 3);
        w.bombs.insert(1, bomb(1, 9, 1, 1, 50, 0));
        let iv = danger_intervals(&w, &Rules::default(), None, 0);
        assert!(iv.contains_key(&(7, 1)));
    }

    #[test]
    fn runs_through_a_bomb_line_when_there_is_time() {
        let mut w = open_field(9, 5);
        for x in 1..7 {
            w.set_tile_for_test(x, 2, Tile::Solid);
        }
        w.players.insert(0, player(0, 3, 1, 2));
        w.players.insert(1, player(1, 1, 3, 6));
        w.bombs.insert(1, bomb(1, 1, 1, 1, 60, 0));
        assert_eq!(decide(&mut Bot::default(), &w), Action::Right);
    }

    #[test]
    fn no_bomb_when_the_escape_is_too_slow() {
        let mut tiles = vec![Tile::Solid; 14 * 3];
        for x in 1..13 {
            tiles[14 + x] = Tile::Empty;
        }
        tiles[14 + 12] = Tile::Soft;
        let mut w = World::from_tiles(14, 3, tiles);
        w.players.insert(0, player(0, 11, 1, 6));
        let slow = Rules {
            bomb_fuse_ticks: 30,
            ..Rules::default()
        };
        let mut bot = Bot::default();
        assert!(!is_bomb(bot.decide(&situation(&w, slow, None, None, 0.0))));
        let mut bot = Bot::default();
        assert!(is_combo(bot.decide(&situation(
            &w,
            Rules::default(),
            None,
            None,
            0.0
        ))));
    }

    #[test]
    fn a_bomb_is_not_repeated_blindly() {
        let mut w = open_field(7, 5);
        w.set_tile_for_test(4, 2, Tile::Soft);
        w.players.insert(0, player(0, 3, 2, 2));
        let mut bot = Bot::default();
        let at = |t| situation(&w, Rules::default(), Some(t), None, 0.0);
        let first = bot.decide(&at(100));
        assert!(is_combo(first));
        assert_eq!(bot.decide(&at(100)), without_bomb(first).unwrap());
        assert!(
            !is_bomb(bot.decide(&at(102))),
            "cooldown: do not bomb again"
        );
        assert!(is_combo(bot.decide(&at(100 + BOMB_COOLDOWN_TICKS))));
    }

    #[test]
    fn replay_follows_the_plan_by_wall_clock() {
        let mut w = open_field(7, 5);
        w.set_tile_for_test(4, 2, Tile::Soft);
        w.players.insert(0, player(0, 3, 2, 2));
        let mut bot = Bot::default();
        let at = |now| situation(&w, Rules::default(), Some(100), None, now);
        assert_eq!(bot.decide(&at(10.0)), Action::UpBomb);
        assert_eq!(bot.decide(&at(10.0)), Action::Up);
        assert_eq!(bot.decide(&at(10.2)), Action::Left);
        let step_s = 8.0 / 60.0;
        assert_eq!(
            bot.decide(&at(10.0 + 1.0 / 60.0 + step_s * 1.7)),
            Action::Idle
        );
        assert_eq!(bot.decide(&at(15.0)), Action::Idle);
    }

    #[test]
    fn threat_cells_use_enemy_radius_and_capacity() {
        let mut w = open_field(9, 3);
        w.players.insert(0, player(0, 6, 1, 2));
        w.players.insert(1, player(1, 1, 1, 3));
        let threat = threat_cells(&w, 0);
        assert!(threat.contains(&(4, 1)) && !threat.contains(&(5, 1)));
        w.bombs.insert(7, bomb(7, 1, 3, 1, 100, 0));
        assert!(threat_cells(&w, 0).is_empty());
    }

    #[test]
    fn escape_prefers_a_haven_outside_the_enemy_line() {
        let mut w = open_field(5, 5);
        w.players.insert(0, player(0, 2, 2, 2));
        w.players.insert(1, player(1, 1, 1, 2));
        w.flames.insert(
            (2, 2),
            FlameView {
                x: 2,
                y: 2,
                ticks: 10,
                seen_tick: 0,
            },
        );
        let a = decide(&mut Bot::default(), &w);
        assert!(a == Action::Down || a == Action::Right, "{a:?}");
    }

    #[test]
    fn sudden_death_prep_devalues_edge_targets() {
        let rules = Rules {
            round_time_ticks: 10800,
            sudden_death_tick: 7200,
            ..Rules::default()
        };
        let mut w = open_field(9, 7);
        w.set_tile_for_test(1, 3, Tile::Soft);
        w.set_tile_for_test(5, 3, Tile::Soft);
        let me = player(0, 3, 3, 2);
        w.players.insert(0, me);
        let bot = Bot::default();
        let world = Sight::new(&w, &rules, 0, None, 7200 - 300);
        assert!(world.sd_prep);
        let vals = bot.target_values(&world, &me, &[]);
        assert!(vals[&(2, 3)] < vals[&(4, 3)]);
        let calm = Sight::new(&w, &rules, 0, None, 600);
        assert!(!calm.sd_prep);
        let vals = bot.target_values(&calm, &me, &[]);
        assert_eq!(vals[&(2, 3)], vals[&(4, 3)]);
    }

    #[test]
    fn sudden_death_moves_inward() {
        let rules = Rules {
            round_time_ticks: 10800,
            sudden_death_tick: 7200,
            ..Rules::default()
        };
        let mut w = open_field(7, 5);
        w.players.insert(0, player(0, 1, 1, 2));
        let mut bot = Bot::default();
        let a = bot.decide(&situation(&w, rules, Some(7200 - 30), None, 0.0));
        assert!(a == Action::Right || a == Action::Down, "{a:?}");
    }

    #[test]
    fn no_bomb_right_after_sending_a_move() {
        let mut w = open_field(7, 5);
        w.set_tile_for_test(4, 2, Tile::Soft);
        w.players.insert(0, player(0, 3, 2, 2));
        let mut bot = Bot::default();
        bot.memo.last_move_tick = Some(99);
        assert!(!is_bomb(bot.decide(&situation(
            &w,
            Rules::default(),
            Some(100),
            None,
            0.0
        ))));
        assert!(is_combo(bot.decide(&situation(
            &w,
            Rules::default(),
            Some(99 + MOVE_SETTLE_TICKS),
            None,
            0.0
        ))));
    }

    #[test]
    fn prefers_a_reachable_powerup() {
        let mut w = open_field(9, 5);
        w.players.insert(0, player(0, 2, 2, 2));
        w.powerups.insert(
            1,
            PowerupView {
                id: 1,
                x: 5,
                y: 2,
                kind: PowerupKind::Flame,
            },
        );
        assert_eq!(decide(&mut Bot::default(), &w), Action::Right);
    }

    fn crate_corner() -> World {
        let mut w = open_field(6, 6);
        w.set_tile_for_test(1, 3, Tile::Soft);
        w.players.insert(0, player(0, 1, 2, 1));
        w
    }

    #[test]
    fn memory_from_the_previous_match_does_not_block_bombing() {
        let w = crate_corner();
        let mut bot = Bot::default();
        bot.memo.tick = Some(4000);
        bot.memo.bomb_sent_tick = Some(3990);
        bot.memo.last_move_tick = Some(3999);
        bot.memo.match_id = Some(1);
        bot.memo.banned.insert((1, 2), 4100);
        let a = bot.decide(&situation(&w, Rules::default(), Some(30), Some(2), 0.0));
        assert!(is_bomb(a));
        assert_eq!(bot.memo.match_id, Some(2));
    }

    #[test]
    fn memory_is_kept_within_a_match_and_a_lost_bomb_is_resent_once() {
        let w = crate_corner();
        let mut bot = Bot::default();
        let at = |t| situation(&w, Rules::default(), Some(t), Some(1), 0.0);
        let first = bot.decide(&at(30));
        assert!(is_bomb(first));
        assert_eq!(bot.decide(&at(31)), first, "resent once: nothing happened");
        assert!(!is_bomb(bot.decide(&at(32))), "then the cooldown holds");
    }

    #[test]
    fn waits_on_its_bomb_spot_while_its_bomb_burns() {
        let mut w = open_field(7, 7);
        w.set_tile_for_test(1, 3, Tile::Soft);
        w.set_tile_for_test(3, 1, Tile::Soft);
        w.players.insert(0, player(0, 1, 2, 1));
        w.bombs.insert(1, bomb(1, 0, 4, 4, 100, 100));
        let mut bot = Bot::default();
        bot.memo.target = Some((1, 2));
        bot.memo.match_id = Some(1);
        for tick in 100..110 {
            assert_eq!(
                bot.decide(&situation(&w, Rules::default(), Some(tick), Some(1), 0.0)),
                Action::Idle
            );
        }
        assert!(!bot.memo.banned.contains_key(&(1, 2)));
    }
}
