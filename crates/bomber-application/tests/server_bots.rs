//! Seats the server fills with bots of its own, configured from the lobby.

mod common;

use std::sync::{Arc, Mutex};

use bomber_application::ports::{BotFactory, SeatBot};
use bomber_application::{ArenaSession, CommandOutcome, ModeratorCommand, SessionConfig};
use bomber_domain::game::{Event, Rules};
use bomber_domain::lobby::LobbyState;
use bomber_domain::shared::PlayerId;
use bomber_protocol::{Action, LobbyStatus, ServerFrame};
use common::*;

/// What the scripted bots saw, per seat.
#[derive(Default, Clone)]
struct Log(Arc<Mutex<Vec<(u8, &'static str)>>>);

impl Log {
    fn frames_for(&self, seat: u8) -> Vec<&'static str> {
        let log = self.0.lock().unwrap();
        log.iter()
            .filter(|(s, _)| *s == seat)
            .map(|(_, f)| *f)
            .collect()
    }
}

/// Drops one bomb as soon as it holds a state, then idles.
struct Scripted {
    seat: u8,
    log: Log,
    has_state: bool,
    bombed: bool,
}

impl SeatBot for Scripted {
    fn observe(&mut self, frame: &ServerFrame) {
        let kind = match frame {
            ServerFrame::Assigned(_) => "assigned",
            ServerFrame::LobbyStatus(_) => "lobby",
            ServerFrame::MatchInit(_) => "init",
            ServerFrame::Keyframe(_) => "keyframe",
            ServerFrame::Delta(_) => "delta",
            ServerFrame::MatchEnd(_) => "end",
        };
        self.has_state |= kind == "keyframe";
        self.log.0.lock().unwrap().push((self.seat, kind));
    }

    fn act(&mut self, _session_tick: u32) -> Action {
        if self.has_state && !self.bombed {
            self.bombed = true;
            return Action::Bomb;
        }
        Action::Idle
    }
}

struct Factory(Log);

impl BotFactory for Factory {
    fn create(&self, seat: PlayerId) -> Box<dyn SeatBot> {
        Box::new(Scripted {
            seat: seat.raw(),
            log: self.0.clone(),
            has_state: false,
            bombed: false,
        })
    }
}

fn config(bot_seats: u8) -> SessionConfig {
    SessionConfig {
        countdown_ticks: 1,
        min_players: 2,
        max_players: 4,
        drop_after_ticks: 0,
        bot_seats,
        rules: Rules {
            round_time_ticks: 3600,
            sudden_death_tick: 1800,
            ..Rules::default()
        },
        ..SessionConfig::default()
    }
}

fn with_bots(bot_seats: u8) -> (ArenaSession, Log) {
    let log = Log::default();
    let session =
        ArenaSession::new(config(bot_seats), 0xC0FFEE).with_bots(Box::new(Factory(log.clone())));
    (session, log)
}

fn status(session: &ArenaSession) -> LobbyStatus {
    match session.lobby_status_frame() {
        ServerFrame::LobbyStatus(status) => status,
        other => panic!("expected a lobby status, got {other:?}"),
    }
}

fn seat_bot(session: &mut ArenaSession, seat: u8, enabled: bool) -> CommandOutcome {
    session.execute(ModeratorCommand::SeatBot {
        player: player(seat),
        enabled,
    })
}

#[test]
fn configured_seats_are_filled_once_a_player_is_there() {
    let (mut session, _) = with_bots(0b1000);
    let before = status(&session);
    assert_eq!(
        before.players_connected, 0,
        "bots do not play among themselves"
    );
    assert!(before.details.unwrap().seats[3].bot_fill);

    session.admit(Some("Anna")).unwrap();
    let status = status(&session);
    let details = status.details.unwrap();
    assert!(details.server_bots);
    assert_eq!(status.players_connected, 2);
    let seat = &details.seats[3];
    assert!(seat.occupied && seat.bot && seat.bot_fill);
    assert_eq!(seat.name, "Server-Bot 4");
    assert!(!seat.stale, "a server bot never goes quiet");
}

#[test]
fn one_player_and_a_bot_can_start_a_match() {
    let (mut session, _) = with_bots(0);
    let anna = session.admit(Some("Anna")).unwrap();
    assert!(!session.snapshot().can_start);

    let outcome = session.seat_bot_request(anna, player(1), true, 1).unwrap();
    assert_eq!(outcome, CommandOutcome::Accepted);
    assert!(session.snapshot().can_start);

    assert!(session
        .player_request(anna, Action::Start, 2)
        .unwrap()
        .is_accepted());
    session.tick();
    assert_eq!(session.state(), LobbyState::Running);
    assert_eq!(session.game().unwrap().players.len(), 2);
}

#[test]
fn a_bot_sees_the_match_and_its_moves_reach_the_game() {
    let (mut session, log) = with_bots(0b0010);
    session.admit(None).unwrap();
    assert!(session.execute(ModeratorCommand::Start).is_accepted());

    let mut bombs_by_bot = 0;
    for _ in 0..5 {
        let report = session.tick();
        bombs_by_bot += report
            .events
            .iter()
            .filter(|e| matches!(e, Event::BombPlaced { owner, .. } if *owner == player(1)))
            .count();
    }

    let seen = log.frames_for(1);
    assert!(
        seen.contains(&"init"),
        "MATCH_INIT is unicast to the bot: {seen:?}"
    );
    assert!(seen.contains(&"keyframe"));
    assert!(seen.contains(&"delta"));
    assert!(
        log.frames_for(0).is_empty(),
        "seat 0 is a player, not a bot"
    );
    assert_eq!(bombs_by_bot, 1, "the bot's bomb was placed");
}

#[test]
fn a_player_takes_a_bot_seat_when_the_table_is_full_and_the_bot_returns_later() {
    let (mut session, _) = with_bots(0b1110);
    let anna = session.admit(Some("Anna")).unwrap();
    assert_eq!(status(&session).players_connected, 4);

    let ben = session.admit(Some("Ben")).unwrap();
    assert_eq!(ben, player(1));
    let seat = &status(&session).details.unwrap().seats[1];
    assert!(seat.occupied && !seat.bot && seat.bot_fill);
    assert_eq!(seat.name, "Ben");

    session.execute(ModeratorCommand::Kick(ben));
    let seat = &status(&session).details.unwrap().seats[1];
    assert!(seat.bot, "the bot is back in its seat");
    assert!(session.is_seated(anna));
}

#[test]
fn switching_a_seat_off_removes_its_bot() {
    let (mut session, _) = with_bots(0b0100);
    session.admit(None).unwrap();
    assert!(status(&session).details.unwrap().seats[2].bot);
    assert!(seat_bot(&mut session, 2, false).is_accepted());
    let seat = &status(&session).details.unwrap().seats[2];
    assert!(!seat.occupied && !seat.bot && !seat.bot_fill);
}

#[test]
fn a_seat_held_by_a_player_waits_for_them_to_leave() {
    let (mut session, _) = with_bots(0);
    let anna = session.admit(None).unwrap();
    assert!(seat_bot(&mut session, anna.raw(), true).is_accepted());
    let seat = &status(&session).details.unwrap().seats[0];
    assert!(seat.occupied && !seat.bot && seat.bot_fill);
    assert!(session.is_seated(anna));
}

#[test]
fn bots_cannot_be_changed_during_a_match() {
    let (mut session, _) = with_bots(0b0010);
    let anna = session.admit(None).unwrap();
    session.execute(ModeratorCommand::Start);
    session.tick();
    assert_eq!(session.state(), LobbyState::Running);

    assert!(!seat_bot(&mut session, 2, true).is_accepted());
    assert!(!session
        .seat_bot_request(anna, player(1), false, 1)
        .unwrap()
        .is_accepted());
    assert!(status(&session).details.unwrap().seats[1].bot);
}

#[test]
fn a_seat_freed_mid_match_is_filled_once_the_results_are_up() {
    let (mut session, _) = with_bots(0);
    session.admit(None).unwrap();
    let ben = session.admit(None).unwrap();
    session.admit(None).unwrap();
    assert!(seat_bot(&mut session, ben.raw(), true).is_accepted());
    session.execute(ModeratorCommand::Start);
    session.tick();

    session.execute(ModeratorCommand::Kick(ben));
    assert!(
        !status(&session).details.unwrap().seats[1].occupied,
        "no bot mid-match"
    );

    session.execute(ModeratorCommand::End);
    session.tick();
    assert_eq!(session.state(), LobbyState::MatchOver);
    assert!(status(&session).details.unwrap().seats[1].bot);
}

#[test]
fn kicking_a_bot_switches_its_seat_off() {
    let (mut session, _) = with_bots(0b0010);
    session.admit(None).unwrap();
    assert!(session
        .execute(ModeratorCommand::Kick(player(1)))
        .is_accepted());
    session.tick();
    let seat = &status(&session).details.unwrap().seats[1];
    assert!(!seat.occupied && !seat.bot_fill);
}

#[test]
fn player_requests_follow_the_player_control_rules() {
    let config = SessionConfig {
        player_control: false,
        ..config(0)
    };
    let mut session = ArenaSession::new(config, 1).with_bots(Box::new(Factory(Log::default())));
    let anna = session.admit(None).unwrap();
    assert!(!session
        .seat_bot_request(anna, player(1), true, 1)
        .unwrap()
        .is_accepted());

    let (mut session, _) = with_bots(0);
    let anna = session.admit(None).unwrap();
    session.execute(ModeratorCommand::Lock);
    assert!(
        !session
            .seat_bot_request(anna, player(1), true, 1)
            .unwrap()
            .is_accepted(),
        "a locked lobby is the moderator's"
    );
    assert!(seat_bot(&mut session, 1, true).is_accepted());
    assert!(status(&session).details.unwrap().seats[1].bot);

    assert!(
        session.seat_bot_request(anna, player(1), true, 1).is_err(),
        "same seq again is a duplicate"
    );
    assert!(
        session
            .seat_bot_request(player(3), player(1), true, 2)
            .is_err(),
        "an empty seat cannot ask"
    );
}

#[test]
fn a_server_without_bots_refuses_the_request() {
    let mut session = session(1);
    assert!(!status(&session).details.unwrap().server_bots);
    assert!(!seat_bot(&mut session, 1, true).is_accepted());
    assert!(!session
        .player_request(player(0), Action::SeatBot, 1)
        .unwrap()
        .is_accepted());
}

#[test]
fn when_the_last_player_leaves_the_bots_leave_too() {
    let (mut session, _) = with_bots(0b0100);
    let anna = session.admit(None).unwrap();
    assert!(seat_bot(&mut session, 1, true).is_accepted());
    assert!(seat_bot(&mut session, 2, false).is_accepted());
    assert_eq!(status(&session).players_connected, 2);

    session.execute(ModeratorCommand::Kick(anna));
    let status_after = status(&session);
    assert_eq!(status_after.players_connected, 0, "no bot stays behind");
    let seats = status_after.details.unwrap().seats;
    assert!(
        !seats[1].bot_fill,
        "the choices made in the lobby are forgotten"
    );
    assert!(seats[2].bot_fill, "the configured seats are back");

    // The next player to arrive finds the configured table.
    session.admit(None).unwrap();
    let seats = status(&session).details.unwrap().seats;
    assert!(!seats[1].occupied);
    assert!(seats[2].bot);
}

#[test]
fn a_match_left_to_the_bots_is_aborted() {
    let (mut session, _) = with_bots(0b0110);
    let anna = session.admit(None).unwrap();
    session.execute(ModeratorCommand::Start);
    session.tick();
    assert_eq!(session.state(), LobbyState::Running);

    session.execute(ModeratorCommand::Kick(anna));
    assert_eq!(session.state(), LobbyState::MatchOver);
    let report = session.tick();
    assert!(report
        .broadcast
        .iter()
        .any(|f| matches!(f, ServerFrame::MatchEnd(_))));
    assert_eq!(status(&session).players_connected, 0);
}

fn dropping_after(ticks: u32) -> ArenaSession {
    let config = SessionConfig {
        drop_after_ticks: ticks,
        ..config(0b0010)
    };
    ArenaSession::new(config, 1).with_bots(Box::new(Factory(Log::default())))
}

#[test]
fn a_silent_player_loses_the_seat_and_the_bots_go_with_them() {
    let mut session = dropping_after(10);
    let anna = session.admit(None).unwrap();
    let ben = session.admit(None).unwrap();
    for tick in 1..=30u8 {
        // Ben keeps talking, Anna has gone.
        session.submit(ben, Action::Idle, tick & 0x0F).unwrap();
        let report = session.tick();
        if tick == 11 {
            assert_eq!(
                report.released,
                vec![anna],
                "dropped once the limit is passed"
            );
        } else {
            assert!(
                report.released.is_empty(),
                "tick {tick}: {:?}",
                report.released
            );
        }
    }
    assert!(!session.is_seated(anna));
    assert!(session.is_seated(ben));
    assert!(
        status(&session).details.unwrap().seats[1].bot,
        "Ben still has company"
    );

    // Now Ben goes quiet too: he is dropped, and the bot leaves with him.
    let mut released = Vec::new();
    for _ in 0..12 {
        released.extend(session.tick().released);
    }
    assert_eq!(released, vec![ben]);
    assert_eq!(status(&session).players_connected, 0);
}

#[test]
fn a_repeated_hello_counts_as_a_sign_of_life() {
    let mut session = dropping_after(10);
    let anna = session.admit(None).unwrap();
    for _ in 0..30 {
        session.heard_from(anna);
        assert!(session.tick().released.is_empty());
    }
    assert!(session.is_seated(anna));
}
