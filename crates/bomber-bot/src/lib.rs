//! The built-in bot, without a window or a socket.
//!
//! [`world`] rebuilds the match from keyframes and deltas, [`bot`] picks the
//! next action from that. Both are pure, so the player client uses them for
//! its "let the bot play" mode and the server uses them to fill empty seats.
//!
//! [`Driver`] is the glue for anyone who has frames rather than a world: feed
//! it every frame addressed to the seat and ask it for an action once a tick.

pub mod bot;
pub mod world;

use bomber_domain::lobby::LobbyState;
use bomber_protocol::{Action, MatchInit, ServerFrame, TICK_RATE};

use bot::{Bot, Situation};
use world::World;

/// One seat played by the bot, fed with the same frames a networked client
/// would receive.
///
/// Loss handling is the one from `BOT_GUIDE.md` §6 -- a keyframe is always
/// trusted, a delta only on exactly its base tick -- even though an in-process
/// seat never loses a frame. It keeps the driver honest if it is ever fed from
/// a real socket.
#[derive(Debug, Default)]
pub struct Driver {
    bot: Bot,
    init: Option<MatchInit>,
    world: Option<World>,
    at_tick: Option<u32>,
    /// Between `MATCH_INIT` and `MATCH_END`.
    playing: bool,
}

impl Driver {
    pub fn new() -> Driver {
        Driver::default()
    }

    pub fn observe(&mut self, frame: &ServerFrame) {
        match frame {
            ServerFrame::Assigned(_) => {}
            ServerFrame::LobbyStatus(status) => {
                if !matches!(status.state, LobbyState::Running | LobbyState::MatchOver) {
                    self.playing = false;
                    self.world = None;
                    self.at_tick = None;
                }
            }
            ServerFrame::MatchInit(init) => {
                let repeat = self
                    .init
                    .as_ref()
                    .is_some_and(|old| old.match_id == init.match_id);
                if !repeat {
                    self.world = None;
                    self.at_tick = None;
                    self.bot.reset();
                }
                self.init = Some(init.clone());
                self.playing = true;
            }
            ServerFrame::Keyframe(keyframe) => {
                if self.init.is_none() || self.at_tick.is_some_and(|t| keyframe.tick < t) {
                    return;
                }
                self.world = Some(World::from_keyframe(keyframe));
                self.at_tick = Some(keyframe.tick);
            }
            ServerFrame::Delta(delta) => {
                let Some(world) = self.world.as_mut() else {
                    return;
                };
                if self.at_tick != Some(delta.base_tick) {
                    return;
                }
                world.apply(delta.tick, &delta.records, &mut Vec::new());
                self.at_tick = Some(delta.tick);
            }
            ServerFrame::MatchEnd(_) => self.playing = false,
        }
    }

    /// The action for this tick. `session_tick` stands in for the wall clock
    /// the bot uses to dead-reckon across a lost delta.
    pub fn act(&mut self, session_tick: u32) -> Action {
        let (Some(init), Some(world)) = (&self.init, &self.world) else {
            return Action::Idle;
        };
        if !self.playing {
            return Action::Idle;
        }
        self.bot.decide(&Situation {
            world,
            rules: init.rules,
            my_id: init.your_player_id.raw(),
            at_tick: self.at_tick,
            match_id: Some(init.match_id),
            now: f64::from(session_tick) / f64::from(TICK_RATE),
        })
    }
}
