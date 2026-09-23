//! The server's own bots: the player client's built-in bot, seated in-process.

use bomber_application::ports::{BotFactory, SeatBot};
use bomber_bot::Driver;
use bomber_domain::shared::PlayerId;
use bomber_protocol::{Action, ServerFrame};

/// Hands out a fresh built-in bot for every seat that asks for one.
pub struct BuiltinBots;

impl BotFactory for BuiltinBots {
    fn create(&self, _seat: PlayerId) -> Box<dyn SeatBot> {
        Box::new(BuiltinBot(Driver::new()))
    }
}

struct BuiltinBot(Driver);

impl SeatBot for BuiltinBot {
    fn observe(&mut self, frame: &ServerFrame) {
        self.0.observe(frame);
    }

    fn act(&mut self, session_tick: u32) -> Action {
        self.0.act(session_tick)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use bomber_application::{ArenaSession, ModeratorCommand, SessionConfig};
    use bomber_domain::game::{Event, Rules};
    use bomber_domain::lobby::LobbyState;

    use super::*;

    /// Three server bots and a player who never touches the keys play a whole
    /// match: the bots move, bomb, and the match ends. Also a budget check,
    /// since the bots think inside the 60 Hz tick.
    #[test]
    fn three_server_bots_play_a_match_to_the_end() {
        let config = SessionConfig {
            countdown_ticks: 1,
            bot_seats: 0b1110,
            drop_after_ticks: 0,
            rules: Rules {
                round_time_ticks: 60 * 60,
                sudden_death_tick: 30 * 60,
                ..Rules::default()
            },
            ..SessionConfig::default()
        };
        let mut session = ArenaSession::new(config, 7).with_bots(Box::new(BuiltinBots));
        session.admit(Some("AFK")).unwrap();
        assert!(session.execute(ModeratorCommand::Start).is_accepted());

        let (mut steps, mut bombs, mut ticks) = (0, 0, 0u32);
        let mut slowest = Duration::ZERO;
        let mut ended = None;
        while ended.is_none() && ticks < 70 * 60 {
            let started = Instant::now();
            let report = session.tick();
            slowest = slowest.max(started.elapsed());
            ticks += 1;
            for event in &report.events {
                match event {
                    Event::PlayerStateChanged { moving: true, .. } => steps += 1,
                    Event::BombPlaced { .. } => bombs += 1,
                    _ => {}
                }
            }
            ended = report.match_ended;
        }

        let outcome = ended.expect("the match ends within its round time");
        assert_eq!(session.state(), LobbyState::MatchOver);
        assert!(steps > 100, "the bots walk: {steps} steps");
        assert!(bombs > 5, "the bots bomb: {bombs} bombs");
        eprintln!(
            "{ticks} ticks, {steps} steps, {bombs} bombs, {:?}, slowest tick {slowest:?}",
            outcome.reason
        );
    }
}
