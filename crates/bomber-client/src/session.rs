//! One connection to the arena: joining, following the lobby, tracking the
//! match, and deciding what goes up the wire each tick.
//!
//! Loss handling follows `BOT_GUIDE.md` §6 to the letter: a keyframe is always
//! trusted, a delta only when we hold exactly its base tick, and anything else
//! waits for the next keyframe. There is no resync to ask for.

use std::collections::VecDeque;

use bomber_domain::game::Rules;
use bomber_domain::lobby::LobbyState;
use bomber_domain::shared::PlayerId;
use bomber_protocol::{Action, LobbyStatus, MatchEnd, MatchInit, ServerFrame, TICK_RATE};

use crate::net::Link;
use crate::world::{FxEvent, World};

/// Resend the hello this often until a seat is assigned.
const HELLO_INTERVAL: f64 = 0.5;
/// No frame for this long means the server is gone.
const LOST_AFTER: f64 = 3.0;
/// While lost, knock again this often -- the server may simply have restarted.
const REJOIN_INTERVAL: f64 = 1.0;
/// Running, but no usable state for this long: ask for MATCH_INIT again.
const NO_STATE_REHELLO: f64 = 1.0;
/// Idle heartbeat, so the lobby can tell a quiet player from a crashed one.
const HEARTBEAT: f64 = 0.5;
/// Each lobby request goes out this many times on consecutive ticks. All four
/// requests are idempotent on the server, and every copy has a fresh sequence
/// number, so this only buys delivery.
const REQUEST_COPIES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Hello sent, no seat yet.
    Connecting,
    Lobby,
    Playing,
    MatchOver,
    /// Nothing heard for a while.
    Lost,
}

/// A lobby request waiting to go out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    /// Start, Pause, Resume or Abort.
    Match(Action),
    /// Fill a seat with a server bot, or remove it.
    SeatBot { seat: u8, enabled: bool },
}

/// Something the presentation layer should react to once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cue {
    Fx(FxEvent),
    MatchStarted,
    MatchEnded,
    /// The start countdown crossed into a new second (3, 2, 1).
    CountdownSecond(u32),
}

pub struct Session {
    link: Link,
    pub name: String,
    pub phase: Phase,
    pub my_id: Option<u8>,
    pub lobby: Option<LobbyStatus>,
    pub init: Option<MatchInit>,
    pub world: Option<World>,
    pub at_tick: Option<u32>,
    pub result: Option<MatchEnd>,
    pub cues: Vec<Cue>,
    started_at: f64,
    last_frame: Option<f64>,
    last_hello: f64,
    last_sent: f64,
    /// Wall time the current world state arrived, for sub-tick smoothing.
    state_time: f64,
    running_without_state_since: Option<f64>,
    requests: VecDeque<Request>,
    last_countdown_second: Option<u32>,
}

impl Session {
    pub fn new(link: Link, name: String, now: f64) -> Session {
        link.send_hello(&name);
        Session {
            link,
            name,
            phase: Phase::Connecting,
            my_id: None,
            lobby: None,
            init: None,
            world: None,
            at_tick: None,
            result: None,
            cues: Vec::new(),
            started_at: now,
            last_frame: None,
            last_hello: now,
            last_sent: now,
            state_time: now,
            running_without_state_since: None,
            requests: VecDeque::new(),
            last_countdown_second: None,
        }
    }

    pub fn server_label(&self) -> String {
        self.link.server().to_string()
    }

    pub fn seconds_connecting(&self, now: f64) -> f64 {
        now - self.started_at
    }

    pub fn rules(&self) -> Rules {
        self.init.as_ref().map(|i| i.rules).unwrap_or_default()
    }

    pub fn paused(&self) -> bool {
        self.lobby
            .as_ref()
            .and_then(|l| l.details.as_ref())
            .is_some_and(|d| d.paused)
    }

    pub fn lobby_state(&self) -> Option<LobbyState> {
        self.lobby.as_ref().map(|l| l.state)
    }

    /// The name shown for a seat. Falls back to the server's default.
    pub fn name_of(&self, id: u8) -> String {
        self.lobby
            .as_ref()
            .and_then(|l| l.details.as_ref())
            .and_then(|d| d.seats.get(id as usize))
            .map(|s| s.name.clone())
            .unwrap_or_else(|| format!("bot-{id}"))
    }

    /// Fraction of a tick elapsed since the last state arrived, 0..1.
    pub fn sub_tick(&self, now: f64) -> f32 {
        if self.paused() {
            return 0.0;
        }
        (((now - self.state_time) * TICK_RATE as f64) as f32).clamp(0.0, 1.0)
    }

    // -- incoming ------------------------------------------------------------

    /// Drain the socket and run the timers. Call every frame.
    pub fn pump(&mut self, now: f64) {
        for frame in self.link.receive() {
            self.on_frame(frame, now);
        }

        let silent_for = self.last_frame.map(|t| now - t);
        match self.phase {
            Phase::Connecting => {
                if now - self.last_hello >= HELLO_INTERVAL {
                    self.hello(now);
                }
            }
            Phase::Lost => {
                if now - self.last_hello >= REJOIN_INTERVAL {
                    self.hello(now);
                }
            }
            _ => {
                if silent_for.is_some_and(|s| s > LOST_AFTER) {
                    self.phase = Phase::Lost;
                    self.hello(now);
                }
            }
        }

        // MATCH_INIT lost five times over, or joined mid-match: ask again.
        if self.lobby_state() == Some(LobbyState::Running) && self.world.is_none() {
            let since = *self.running_without_state_since.get_or_insert(now);
            if now - since > NO_STATE_REHELLO && now - self.last_hello > NO_STATE_REHELLO {
                self.hello(now);
            }
        } else {
            self.running_without_state_since = None;
        }
    }

    fn hello(&mut self, now: f64) {
        self.link.send_hello(&self.name);
        self.last_hello = now;
    }

    pub fn on_frame(&mut self, frame: ServerFrame, now: f64) {
        self.last_frame = Some(now);
        if self.phase == Phase::Lost {
            // Heard from again; LOBBY_STATUS will say where we are.
            self.phase = if self.my_id.is_some() {
                Phase::Lobby
            } else {
                Phase::Connecting
            };
        }

        match frame {
            ServerFrame::Assigned(assigned) => {
                self.my_id = Some(assigned.player_id.raw());
                if self.phase == Phase::Connecting {
                    self.phase = Phase::Lobby;
                }
            }
            ServerFrame::LobbyStatus(status) => self.on_lobby(status),
            ServerFrame::MatchInit(init) => {
                let repeat = self
                    .init
                    .as_ref()
                    .is_some_and(|old| old.match_id == init.match_id)
                    && self.world.is_some();
                self.my_id = Some(init.your_player_id.raw());
                if !repeat {
                    // A new match. MATCH_INIT is repeated on five ticks; a
                    // repeat of the same one must not wipe a state we hold.
                    self.world = None;
                    self.at_tick = None;
                    self.result = None;
                    self.cues.push(Cue::MatchStarted);
                }
                self.init = Some(init);
                self.phase = Phase::Playing;
            }
            ServerFrame::Keyframe(keyframe) => {
                if self.init.is_none() {
                    return;
                }
                // A reordered keyframe from the past must not rewind us.
                if self.at_tick.is_some_and(|t| keyframe.tick < t) {
                    return;
                }
                self.world = Some(World::from_keyframe(&keyframe));
                self.at_tick = Some(keyframe.tick);
                self.state_time = now;
                if self.phase != Phase::MatchOver {
                    self.phase = Phase::Playing;
                }
            }
            ServerFrame::Delta(delta) => {
                let Some(world) = self.world.as_mut() else {
                    return;
                };
                if self.at_tick != Some(delta.base_tick) {
                    return; // a gap: wait for the next keyframe
                }
                let mut fx = Vec::new();
                world.apply(delta.tick, &delta.records, &mut fx);
                self.cues.extend(fx.into_iter().map(Cue::Fx));
                self.at_tick = Some(delta.tick);
                self.state_time = now;
            }
            ServerFrame::MatchEnd(end) => {
                if self.result.is_none() {
                    self.cues.push(Cue::MatchEnded);
                }
                self.result = Some(end);
                self.phase = Phase::MatchOver;
            }
        }
    }

    fn on_lobby(&mut self, status: LobbyStatus) {
        match status.state {
            LobbyState::Open | LobbyState::Locked | LobbyState::Countdown => {
                self.phase = Phase::Lobby;
                self.world = None;
                self.at_tick = None;
                self.result = None;
                if status.state == LobbyState::Countdown {
                    let second = (status.countdown_ticks as u32).div_ceil(TICK_RATE as u32);
                    if second > 0 && self.last_countdown_second != Some(second) {
                        self.cues.push(Cue::CountdownSecond(second));
                    }
                    self.last_countdown_second = Some(second);
                } else {
                    self.last_countdown_second = None;
                }
            }
            LobbyState::Running => {
                self.last_countdown_second = None;
                if self.phase != Phase::MatchOver && self.init.is_some() {
                    self.phase = Phase::Playing;
                }
            }
            LobbyState::MatchOver => {
                // Also covers an abort from an older server, which sends no
                // MATCH_END: the lobby state is then the only word we get.
                if self.phase == Phase::Playing && self.result.is_none() {
                    self.cues.push(Cue::MatchEnded);
                }
                self.phase = Phase::MatchOver;
            }
        }
        self.lobby = Some(status);
    }

    // -- outgoing ------------------------------------------------------------

    /// Queue a lobby request (Start, Pause, Resume, Abort).
    pub fn request(&mut self, action: Action) {
        debug_assert!(action.is_lobby_request() && action != Action::SeatBot);
        self.enqueue(Request::Match(action));
    }

    /// Queue a request to fill a seat with a server bot, or to remove it.
    /// Setting rather than toggling, so the repeated copies are harmless.
    pub fn request_seat_bot(&mut self, seat: u8, enabled: bool) {
        // A newer choice for the same seat replaces one still queued.
        self.requests
            .retain(|r| !matches!(r, Request::SeatBot { seat: s, .. } if *s == seat));
        self.enqueue(Request::SeatBot { seat, enabled });
    }

    fn enqueue(&mut self, request: Request) {
        self.requests.retain(|r| *r != request);
        for _ in 0..REQUEST_COPIES {
            self.requests.push_back(request);
        }
    }

    /// Send this tick's packet, if there is anything worth sending. Call once
    /// per game tick with the move the player (or the bot) wants.
    pub fn send_tick(&mut self, action: Action, now: f64) {
        let Some(id) = self.my_id else {
            return;
        };
        if matches!(self.phase, Phase::Connecting | Phase::Lost) {
            return;
        }
        let player = PlayerId::new(id);
        let action = match self.requests.pop_front() {
            Some(Request::Match(request)) => request,
            Some(Request::SeatBot { seat, enabled }) => {
                self.link.send_seat_bot(player, seat, enabled);
                self.last_sent = now;
                return;
            }
            None => action,
        };
        if action != Action::Idle {
            self.link.send_action(player, action);
            self.last_sent = now;
        } else if now - self.last_sent >= HEARTBEAT {
            self.link.send_action(player, Action::Idle);
            self.last_sent = now;
        }
    }

    /// Whether the local player may press Start right now, as far as the
    /// server has told us.
    pub fn can_request_start(&self) -> bool {
        let Some(lobby) = &self.lobby else {
            return false;
        };
        let Some(details) = &lobby.details else {
            return false;
        };
        if !details.player_control || details.paused {
            return false;
        }
        match lobby.state {
            LobbyState::Open | LobbyState::Locked => details.can_start,
            // Start on the results screen means "next match".
            LobbyState::MatchOver => lobby.players_connected >= details.min_players,
            _ => false,
        }
    }

    /// Whether the local player may switch server bots on seats right now.
    pub fn can_change_bots(&self) -> bool {
        let Some(lobby) = &self.lobby else {
            return false;
        };
        let Some(details) = &lobby.details else {
            return false;
        };
        details.server_bots
            && details.player_control
            && !details.paused
            && matches!(lobby.state, LobbyState::Open | LobbyState::MatchOver)
    }

    pub fn player_control(&self) -> bool {
        self.lobby
            .as_ref()
            .and_then(|l| l.details.as_ref())
            .is_some_and(|d| d.player_control)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bomber_domain::board::{Tile, TileGrid};
    use bomber_domain::game::EndReason;
    use bomber_domain::shared::{Cell, PlayerId};
    use bomber_protocol::{
        Assigned, Delta, DeltaRecord, Keyframe, LobbyDetails, SeatStatus, PROTOCOL_VERSION,
    };

    fn session() -> Session {
        // A socket to a port nobody listens on; only on_frame is exercised.
        let link = Link::open("127.0.0.1", 9).unwrap();
        Session::new(link, "Test".into(), 0.0)
    }

    fn lobby(state: LobbyState, can_start: bool) -> ServerFrame {
        ServerFrame::LobbyStatus(LobbyStatus {
            tick: 1,
            state,
            players_connected: 2,
            max_players: 4,
            slot_mask: 0b11,
            countdown_ticks: if state == LobbyState::Countdown {
                150
            } else {
                0
            },
            details: Some(LobbyDetails {
                paused: false,
                can_start,
                player_control: true,
                server_bots: true,
                min_players: 2,
                seats: (0..4)
                    .map(|i| SeatStatus {
                        occupied: i < 2,
                        stale: false,
                        bot: false,
                        bot_fill: false,
                        name: format!("P{i}"),
                    })
                    .collect(),
            }),
        })
    }

    fn init(match_id: u32) -> ServerFrame {
        ServerFrame::MatchInit(MatchInit {
            tick: 0,
            protocol_version: PROTOCOL_VERSION,
            match_id,
            seed: 1,
            your_player_id: PlayerId::new(1),
            grid: TileGrid::filled(5, 5, Tile::Empty),
            spawns: vec![Cell::new(1, 1), Cell::new(3, 3)],
            rules: Rules::default(),
        })
    }

    fn keyframe(tick: u32) -> ServerFrame {
        ServerFrame::Keyframe(Keyframe {
            tick,
            grid: TileGrid::filled(5, 5, Tile::Empty),
            players: vec![],
            bombs: vec![],
            flames: vec![],
            powerups: vec![],
            ticks_remaining: 100,
        })
    }

    fn delta(tick: u32, base: u32) -> ServerFrame {
        ServerFrame::Delta(Delta {
            tick,
            base_tick: base,
            records: vec![DeltaRecord::Timer {
                ticks_remaining: 99,
            }],
        })
    }

    #[test]
    fn joining_moves_from_connecting_to_lobby() {
        let mut s = session();
        assert_eq!(s.phase, Phase::Connecting);
        s.on_frame(
            ServerFrame::Assigned(Assigned {
                tick: 0,
                protocol_version: PROTOCOL_VERSION,
                player_id: PlayerId::new(1),
                tick_rate: 60,
                max_players: 4,
            }),
            0.1,
        );
        assert_eq!(s.phase, Phase::Lobby);
        assert_eq!(s.my_id, Some(1));
        s.on_frame(lobby(LobbyState::Open, true), 0.2);
        assert!(s.can_request_start());
        assert!(s.can_change_bots());
        assert_eq!(s.name_of(0), "P0");
    }

    #[test]
    fn a_newer_seat_bot_choice_replaces_a_queued_one() {
        let mut s = session();
        s.request_seat_bot(2, true);
        s.request_seat_bot(3, true);
        s.request_seat_bot(2, false);
        let queued: Vec<_> = s.requests.iter().copied().collect();
        assert_eq!(queued.len(), 2 * REQUEST_COPIES);
        assert!(!queued.contains(&Request::SeatBot {
            seat: 2,
            enabled: true
        }));
    }

    #[test]
    fn a_delta_applies_only_on_its_base_tick() {
        let mut s = session();
        s.on_frame(init(1), 0.0);
        s.on_frame(keyframe(30), 0.0);
        s.on_frame(delta(32, 31), 0.0); // gap: 31 missing
        assert_eq!(s.at_tick, Some(30));
        s.on_frame(delta(31, 30), 0.0);
        assert_eq!(s.at_tick, Some(31));
        assert_eq!(s.world.as_ref().unwrap().ticks_remaining, 99);
    }

    #[test]
    fn a_repeated_match_init_keeps_the_state() {
        let mut s = session();
        s.on_frame(init(7), 0.0);
        s.on_frame(keyframe(0), 0.0);
        s.on_frame(init(7), 0.0);
        assert!(s.world.is_some(), "same match id: keep the state");
        s.on_frame(init(8), 0.0);
        assert!(s.world.is_none(), "a new match starts clean");
        let starts = s.cues.iter().filter(|c| **c == Cue::MatchStarted).count();
        assert_eq!(starts, 2);
    }

    #[test]
    fn a_stale_keyframe_does_not_rewind() {
        let mut s = session();
        s.on_frame(init(1), 0.0);
        s.on_frame(keyframe(60), 0.0);
        s.on_frame(keyframe(30), 0.0);
        assert_eq!(s.at_tick, Some(60));
    }

    #[test]
    fn match_end_shows_results_until_the_lobby_reopens() {
        let mut s = session();
        s.on_frame(init(1), 0.0);
        s.on_frame(keyframe(0), 0.0);
        s.on_frame(
            ServerFrame::MatchEnd(MatchEnd {
                tick: 5,
                reason: EndReason::Aborted,
                winner: None,
                results: vec![],
            }),
            0.0,
        );
        assert_eq!(s.phase, Phase::MatchOver);
        s.on_frame(lobby(LobbyState::MatchOver, false), 0.0);
        assert_eq!(s.phase, Phase::MatchOver, "results stay up");
        assert!(s.can_request_start(), "Start means next match");
        s.on_frame(lobby(LobbyState::Countdown, false), 0.0);
        assert_eq!(s.phase, Phase::Lobby);
        assert!(s.result.is_none());
        assert!(s.cues.contains(&Cue::CountdownSecond(3)));
    }

    #[test]
    fn silence_means_lost() {
        let mut s = session();
        s.on_frame(lobby(LobbyState::Open, true), 0.0);
        s.pump(1.0);
        assert_eq!(s.phase, Phase::Lobby);
        s.pump(3.5);
        assert_eq!(s.phase, Phase::Lost);
    }
}
