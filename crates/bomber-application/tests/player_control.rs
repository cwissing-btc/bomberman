//! Seated players starting, pausing and aborting matches from their own client.

mod common;

use bomber_application::{ArenaSession, CommandOutcome, ModeratorCommand, SessionConfig};
use bomber_domain::game::EndReason;
use bomber_domain::lobby::LobbyState;
use bomber_protocol::{Action, ServerFrame};
use common::*;

fn lobby_status(frames: &[ServerFrame]) -> Option<&bomber_protocol::LobbyStatus> {
    frames.iter().find_map(|f| match f {
        ServerFrame::LobbyStatus(status) => Some(status),
        _ => None,
    })
}

#[test]
fn a_seated_player_can_start_the_match() {
    let mut session = session(2);
    let outcome = session.player_request(player(1), Action::Start, 1).unwrap();
    assert_eq!(outcome, CommandOutcome::Accepted);
    assert_eq!(session.state(), LobbyState::Countdown);

    session.tick();
    assert_eq!(session.state(), LobbyState::Running);
}

#[test]
fn a_player_start_with_too_few_players_says_why() {
    let mut session = session(1);
    match session.player_request(player(0), Action::Start, 1).unwrap() {
        CommandOutcome::Rejected(reason) => assert!(reason.contains("need at least 2"), "{reason}"),
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert_eq!(session.state(), LobbyState::Open);
}

/// UDP may deliver one request twice. The sequence counter must catch it, so
/// one press is never two -- which matters for anything that is not
/// idempotent.
#[test]
fn a_duplicated_request_runs_once() {
    let mut session = running(2);
    assert!(session
        .player_request(player(0), Action::Pause, 3)
        .unwrap()
        .is_accepted());
    assert!(
        session.player_request(player(0), Action::Pause, 3).is_err(),
        "same seq again is a duplicate"
    );
    assert!(session.is_paused());
}

#[test]
fn an_empty_seat_cannot_send_requests() {
    let mut session = session(2);
    assert!(session.player_request(player(3), Action::Start, 1).is_err());
    assert_eq!(session.state(), LobbyState::Open);
}

/// On the results screen Start means "next match": results are cleared and
/// the countdown begins in one step.
#[test]
fn start_after_a_match_resets_and_counts_down() {
    let mut session = running(2);
    session.execute(ModeratorCommand::End);
    session.tick();
    assert_eq!(session.state(), LobbyState::MatchOver);

    assert!(session
        .player_request(player(0), Action::Start, 1)
        .unwrap()
        .is_accepted());
    assert_eq!(session.state(), LobbyState::Countdown);
    session.tick();
    assert_eq!(session.state(), LobbyState::Running);
}

/// Refusing must not throw away the results as a side effect.
#[test]
fn a_refused_restart_leaves_the_results_up() {
    let mut session = running(2);
    session.execute(ModeratorCommand::End);
    session.tick();
    session.execute(ModeratorCommand::Kick(player(1)));

    let outcome = session.player_request(player(0), Action::Start, 1).unwrap();
    assert!(!outcome.is_accepted());
    assert_eq!(session.state(), LobbyState::MatchOver);
    assert!(session.last_outcome().is_some());
}

#[test]
fn pause_is_refused_in_the_lobby() {
    let mut session = session(2);
    assert!(!session
        .player_request(player(0), Action::Pause, 1)
        .unwrap()
        .is_accepted());
    assert!(!session.is_paused());
}

/// A frozen match clock must not look like a dead server to the clients.
#[test]
fn a_paused_session_keeps_sending_lobby_status() {
    let mut session = running(2);
    session.player_request(player(0), Action::Pause, 1).unwrap();

    let mut heard = 0;
    let mut paused_flag = false;
    for _ in 0..90 {
        let report = session.tick();
        if let Some(status) = lobby_status(&report.broadcast) {
            heard += 1;
            paused_flag = status.details.as_ref().is_some_and(|d| d.paused);
        }
    }
    assert!(heard >= 3, "heard {heard} lobby frames in 1.5 s of pause");
    assert!(paused_flag, "the frames say the match is paused");

    session
        .player_request(player(1), Action::Resume, 1)
        .unwrap();
    assert!(!session.is_paused());
}

/// An aborted match ends the way a finished one does: with a MATCH_END every
/// client can show, instead of one they have to infer from the lobby state.
#[test]
fn abort_sends_match_end_with_reason_aborted() {
    let mut session = running(2);
    assert!(session
        .player_request(player(1), Action::Abort, 1)
        .unwrap()
        .is_accepted());

    let report = session.tick();
    let end = report
        .broadcast
        .iter()
        .find_map(|f| match f {
            ServerFrame::MatchEnd(end) => Some(end),
            _ => None,
        })
        .expect("a MATCH_END goes out on the next tick");
    assert_eq!(end.reason, EndReason::Aborted);
    assert_eq!(
        report.match_ended.map(|o| o.reason),
        Some(EndReason::Aborted)
    );

    // Exactly once.
    let later = session.tick();
    assert!(!later
        .broadcast
        .iter()
        .any(|f| matches!(f, ServerFrame::MatchEnd(_))));
}

#[test]
fn a_command_is_reflected_in_lobby_status_on_the_next_tick() {
    // The default three-second countdown, so the state is still observable.
    let mut session = ArenaSession::new(SessionConfig::default(), 1);
    session.admit(None).unwrap();
    session.admit(None).unwrap();
    session.tick();
    session.player_request(player(0), Action::Start, 1).unwrap();
    let report = session.tick();
    let status = lobby_status(&report.broadcast).expect("lobby status sent at once");
    assert_eq!(status.state, LobbyState::Countdown);
}

#[test]
fn lobby_status_carries_names_and_start_readiness() {
    let mut session = ArenaSession::new(SessionConfig::default(), 1);
    session.admit(Some("Anna")).unwrap();
    session.admit(Some("Ben")).unwrap();

    let ServerFrame::LobbyStatus(status) = session.lobby_status_frame() else {
        panic!("not a lobby status");
    };
    let details = status.details.expect("the extended tail is sent");
    assert!(details.can_start);
    assert!(details.player_control);
    assert_eq!(details.min_players, 2);
    assert_eq!(details.seats.len(), 4);
    assert_eq!(details.seats[0].name, "Anna");
    assert!(details.seats[1].occupied);
    assert!(!details.seats[2].occupied);
}

#[test]
fn player_control_can_be_switched_off() {
    let config = SessionConfig {
        player_control: false,
        ..SessionConfig::default()
    };
    let mut session = ArenaSession::new(config, 1);
    session.admit(None).unwrap();
    session.admit(None).unwrap();

    let outcome = session.player_request(player(0), Action::Start, 1).unwrap();
    assert!(!outcome.is_accepted());
    assert_eq!(session.state(), LobbyState::Open);
    // The moderator still can.
    assert!(session.execute(ModeratorCommand::Start).is_accepted());
}

/// A lobby request carries no intent: it must not cancel a move queued in
/// the same tick window.
#[test]
fn a_request_does_not_replace_the_pending_move() {
    let mut session = running(2);
    let before = session.game().unwrap().players[0].cell;
    session.submit(player(0), Action::Right, 1).unwrap();
    // Resume while not paused is refused, so the match keeps running.
    session
        .player_request(player(0), Action::Resume, 2)
        .unwrap();
    session.tick();
    let after = &session.game().unwrap().players[0];
    assert!(
        after.cell != before || after.is_moving(),
        "the RIGHT sent before the request still applied"
    );
}

/// Players can go from results straight to the next match, so a latecomer must
/// be able to join while the results are up -- and then play in that match.
#[test]
fn a_latecomer_joins_on_the_results_screen_and_plays_next() {
    let mut session = running(2);
    assert!(session.admit(Some("late")).is_err(), "no joining mid-match");
    session.execute(ModeratorCommand::End);
    session.tick();

    let late = session
        .admit(Some("late"))
        .expect("seated during the results");
    assert!(session
        .player_request(player(0), Action::Start, 1)
        .unwrap()
        .is_accepted());
    session.tick();
    let game = session.game().expect("the next match runs");
    assert!(game.players.iter().any(|p| p.id == late));
}
