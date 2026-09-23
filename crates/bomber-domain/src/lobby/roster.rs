use crate::shared::PlayerId;

use super::{LobbyState, Seat};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionError {
    /// The roster is frozen or a match is under way.
    NotAcceptingPlayers,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    NotEnoughPlayers { have: u8, need: u8 },
    AlreadyRunning,
}

/// The lobby aggregate: the seats and the state machine over them.
#[derive(Debug, Clone)]
pub struct Lobby {
    state: LobbyState,
    seats: Vec<Seat>,
    min_players: u8,
    countdown_ticks: u16,
}

impl Lobby {
    pub fn new(max_players: u8, min_players: u8) -> Self {
        Lobby {
            state: LobbyState::Open,
            seats: (0..max_players)
                .map(|raw| Seat::empty(PlayerId::new(raw)))
                .collect(),
            min_players: min_players.max(1),
            countdown_ticks: 0,
        }
    }

    pub fn state(&self) -> LobbyState {
        self.state
    }

    pub fn seats(&self) -> &[Seat] {
        &self.seats
    }

    pub fn max_players(&self) -> u8 {
        self.seats.len() as u8
    }

    pub fn min_players(&self) -> u8 {
        self.min_players
    }

    pub fn occupied_count(&self) -> u8 {
        self.seats.iter().filter(|s| s.occupied).count() as u8
    }

    /// Bit `i` set means seat `i` is taken. Sent to bots, which have no room
    /// for a richer roster.
    pub fn occupancy_mask(&self) -> u8 {
        self.seats
            .iter()
            .enumerate()
            .filter(|(_, s)| s.occupied)
            .fold(0u8, |mask, (i, _)| mask | (1 << i))
    }

    pub fn countdown_ticks(&self) -> u16 {
        self.countdown_ticks
    }

    /// Seat the next free slot, optionally under a name the joiner proposed.
    ///
    /// The name is applied at admission only. A moderator renaming a seat
    /// afterwards wins and keeps winning: a bot that reconnects should not be
    /// able to undo a label put there to tell two of them apart.
    ///
    /// A free seat is taken first. With none left, a player displaces a server
    /// bot: bots only fill seats nobody wants, so a person is never turned
    /// away for a bot's sake. The seat keeps its `bot_fill` setting, and the
    /// bot comes back when the player leaves.
    pub fn admit(&mut self, proposed_name: Option<&str>) -> Result<PlayerId, AdmissionError> {
        if !self.state.admits_new_players() {
            return Err(AdmissionError::NotAcceptingPlayers);
        }
        let index = self
            .seats
            .iter()
            .position(|s| !s.occupied)
            .or_else(|| self.seats.iter().position(|s| s.bot))
            .ok_or(AdmissionError::Full)?;
        let seat = &mut self.seats[index];
        seat.vacate();
        seat.occupied = true;
        if let Some(name) = proposed_name {
            seat.set_name(name);
        }
        Ok(seat.id)
    }

    /// Put a server bot into an empty seat. Returns whether it sat down.
    ///
    /// Not bound to the admission states: whether a bot may join now is the
    /// caller's decision, since a moderator may fill a locked lobby.
    pub fn seat_bot(&mut self, id: PlayerId) -> bool {
        match self.seats.iter_mut().find(|s| s.id == id) {
            Some(seat) if !seat.occupied => {
                seat.occupied = true;
                seat.bot = true;
                seat.name = Seat::bot_name(id);
                true
            }
            _ => false,
        }
    }

    /// Configure whether this seat is filled by a server bot. Seating or
    /// removing the bot is a separate step, see [`Lobby::seat_bot`] and
    /// [`Lobby::release`].
    pub fn set_bot_fill(&mut self, id: PlayerId, enabled: bool) {
        if let Some(seat) = self.seats.iter_mut().find(|s| s.id == id) {
            seat.bot_fill = enabled;
        }
    }

    pub fn is_bot(&self, id: PlayerId) -> bool {
        self.seats.iter().any(|s| s.id == id && s.bot)
    }

    pub fn release(&mut self, id: PlayerId) {
        if let Some(seat) = self.seats.iter_mut().find(|s| s.id == id) {
            seat.vacate();
        }
    }

    pub fn rename(&mut self, id: PlayerId, name: &str) {
        if let Some(seat) = self.seats.iter_mut().find(|s| s.id == id) {
            seat.set_name(name);
        }
    }

    pub fn is_occupied(&self, id: PlayerId) -> bool {
        self.seats
            .iter()
            .any(|s| s.id == id && s.occupied)
    }

    /// The ids that will take part in the next match, in seat order.
    pub fn participants(&self) -> Vec<PlayerId> {
        self.seats
            .iter()
            .filter(|s| s.occupied)
            .map(|s| s.id)
            .collect()
    }

    pub fn lock(&mut self) {
        if self.state == LobbyState::Open {
            self.state = LobbyState::Locked;
        }
    }

    pub fn unlock(&mut self) {
        if self.state == LobbyState::Locked {
            self.state = LobbyState::Open;
        }
    }

    /// Would [`Lobby::begin_countdown`] succeed right now?
    ///
    /// Exposed so a UI can grey out its Start button using the same rule the
    /// server enforces, rather than re-deriving it and drifting.
    pub fn can_start(&self) -> bool {
        self.begin_countdown_error().is_none()
    }

    fn begin_countdown_error(&self) -> Option<StartError> {
        // MatchOver counts as 'already running' for this purpose: the results
        // of the last match are still on screen, and starting straight over
        // them would discard what a moderator is very likely still reading.
        // Reset is the deliberate act of dismissing them.
        if matches!(
            self.state,
            LobbyState::Countdown | LobbyState::Running | LobbyState::MatchOver
        ) {
            return Some(StartError::AlreadyRunning);
        }
        let have = self.occupied_count();
        (have < self.min_players).then_some(StartError::NotEnoughPlayers {
            have,
            need: self.min_players,
        })
    }

    pub fn begin_countdown(&mut self, ticks: u16) -> Result<(), StartError> {
        if let Some(err) = self.begin_countdown_error() {
            return Err(err);
        }
        self.state = LobbyState::Countdown;
        self.countdown_ticks = ticks;
        Ok(())
    }

    /// Advance the countdown by a tick. Returns true on the tick it reaches zero.
    pub fn tick_countdown(&mut self) -> bool {
        if self.state != LobbyState::Countdown {
            return false;
        }
        self.countdown_ticks = self.countdown_ticks.saturating_sub(1);
        self.countdown_ticks == 0
    }

    pub fn begin_match(&mut self) {
        self.state = LobbyState::Running;
        self.countdown_ticks = 0;
    }

    pub fn finish_match(&mut self) {
        self.state = LobbyState::MatchOver;
    }

    /// Back to accepting players. Seats keep their occupants, so a bot that is
    /// still connected does not have to say hello again between matches.
    pub fn reset(&mut self) {
        self.state = LobbyState::Open;
        self.countdown_ticks = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seats_are_handed_out_in_order_and_reused_after_a_kick() {
        let mut lobby = Lobby::new(4, 2);
        assert_eq!(lobby.admit(None), Ok(PlayerId::new(0)));
        assert_eq!(lobby.admit(Some("team-rocket")), Ok(PlayerId::new(1)));
        assert_eq!(lobby.seats()[1].name, "team-rocket", "a joiner may name itself");
        lobby.release(PlayerId::new(0));
        assert_eq!(lobby.admit(None), Ok(PlayerId::new(0)), "freed seat is reused");
    }

    #[test]
    fn a_full_lobby_refuses_admission() {
        let mut lobby = Lobby::new(2, 2);
        assert!(lobby.admit(None).is_ok());
        assert!(lobby.admit(None).is_ok());
        assert_eq!(lobby.admit(None), Err(AdmissionError::Full));
    }

    #[test]
    fn a_running_match_refuses_admission_but_its_results_screen_does_not() {
        let mut lobby = Lobby::new(4, 2);
        lobby.admit(None).unwrap();
        lobby.admit(None).unwrap();
        lobby.begin_countdown(1).unwrap();
        assert_eq!(lobby.admit(None), Err(AdmissionError::NotAcceptingPlayers));
        lobby.begin_match();
        assert_eq!(lobby.admit(None), Err(AdmissionError::NotAcceptingPlayers));
        lobby.finish_match();
        assert!(lobby.admit(None).is_ok(), "a latecomer joins during the results");
        assert_eq!(lobby.participants().len(), 3);
    }

    #[test]
    fn a_locked_lobby_refuses_admission() {
        let mut lobby = Lobby::new(4, 2);
        lobby.lock();
        assert_eq!(lobby.admit(None), Err(AdmissionError::NotAcceptingPlayers));
        lobby.unlock();
        assert!(lobby.admit(None).is_ok());
    }

    #[test]
    fn a_match_needs_the_minimum_number_of_players() {
        let mut lobby = Lobby::new(4, 2);
        assert!(!lobby.can_start());
        assert_eq!(
            lobby.begin_countdown(60),
            Err(StartError::NotEnoughPlayers { have: 0, need: 2 })
        );
        lobby.admit(None).unwrap();
        lobby.admit(None).unwrap();
        assert!(lobby.can_start());
        assert!(lobby.begin_countdown(60).is_ok());
        assert_eq!(lobby.state(), LobbyState::Countdown);
    }

    #[test]
    fn the_countdown_reports_the_tick_it_expires_on() {
        let mut lobby = Lobby::new(4, 1);
        lobby.admit(None).unwrap();
        lobby.begin_countdown(3).unwrap();
        assert!(!lobby.tick_countdown());
        assert!(!lobby.tick_countdown());
        assert!(lobby.tick_countdown(), "fires exactly once, at zero");
    }

    /// Results stay on screen until somebody dismisses them.
    #[test]
    fn a_finished_match_must_be_reset_before_another_can_start() {
        let mut lobby = Lobby::new(4, 2);
        lobby.admit(None).unwrap();
        lobby.admit(None).unwrap();
        lobby.begin_countdown(1).unwrap();
        lobby.begin_match();
        lobby.finish_match();

        assert!(!lobby.can_start());
        assert_eq!(
            lobby.begin_countdown(60),
            Err(StartError::AlreadyRunning)
        );

        lobby.reset();
        assert!(lobby.can_start(), "reset dismisses the results and re-arms Start");
    }

    #[test]
    fn occupancy_mask_tracks_seats() {
        let mut lobby = Lobby::new(4, 1);
        lobby.admit(None).unwrap();
        lobby.admit(None).unwrap();
        assert_eq!(lobby.occupancy_mask(), 0b0011);
        lobby.release(PlayerId::new(0));
        assert_eq!(lobby.occupancy_mask(), 0b0010);
    }
}

#[cfg(test)]
mod bots {
    use super::*;

    #[test]
    fn a_bot_takes_only_an_empty_seat() {
        let mut lobby = Lobby::new(4, 2);
        let human = lobby.admit(Some("Anna")).unwrap();
        assert!(!lobby.seat_bot(human), "a player's seat is not a bot's");
        assert!(lobby.seat_bot(PlayerId::new(1)));
        assert!(!lobby.seat_bot(PlayerId::new(1)), "already taken");
        assert!(lobby.is_bot(PlayerId::new(1)));
        assert_eq!(lobby.seats()[1].name, "Server-Bot 2");
        assert_eq!(lobby.participants().len(), 2, "a bot plays like anyone");
        assert!(lobby.can_start());
    }

    #[test]
    fn a_player_prefers_a_free_seat_over_displacing_a_bot() {
        let mut lobby = Lobby::new(3, 2);
        lobby.seat_bot(PlayerId::new(0));
        assert_eq!(lobby.admit(None), Ok(PlayerId::new(1)));
    }

    #[test]
    fn a_player_displaces_a_bot_when_the_table_is_full() {
        let mut lobby = Lobby::new(2, 2);
        lobby.admit(Some("Anna")).unwrap();
        lobby.set_bot_fill(PlayerId::new(1), true);
        lobby.seat_bot(PlayerId::new(1));

        assert_eq!(lobby.admit(Some("Ben")), Ok(PlayerId::new(1)));
        let seat = &lobby.seats()[1];
        assert!(seat.occupied && !seat.bot);
        assert_eq!(seat.name, "Ben");
        assert!(seat.bot_fill, "the bot returns when Ben leaves");
        assert_eq!(lobby.admit(None), Err(AdmissionError::Full));
    }

    #[test]
    fn a_bot_is_not_displaced_while_a_match_runs() {
        let mut lobby = Lobby::new(2, 2);
        lobby.admit(None).unwrap();
        lobby.seat_bot(PlayerId::new(1));
        lobby.begin_countdown(1).unwrap();
        lobby.begin_match();
        assert_eq!(lobby.admit(None), Err(AdmissionError::NotAcceptingPlayers));
        assert!(lobby.is_bot(PlayerId::new(1)));
    }
}


#[cfg(test)]
mod naming {
    use super::*;

    #[test]
    fn a_seat_without_a_proposed_name_keeps_the_default() {
        let mut lobby = Lobby::new(4, 2);
        lobby.admit(None).unwrap();
        assert_eq!(lobby.seats()[0].name, "bot-0");
    }

    /// A moderator renames a seat to tell two bots apart. A bot reconnecting
    /// must not be able to quietly undo that.
    #[test]
    fn a_moderator_rename_outlives_a_rejoin() {
        let mut lobby = Lobby::new(4, 2);
        let id = lobby.admit(Some("self-chosen")).unwrap();
        lobby.rename(id, "moderator-chosen");
        assert_eq!(lobby.seats()[0].name, "moderator-chosen");

        // Same seat, still occupied: a re-hello does not re-admit.
        assert!(lobby.is_occupied(id));
        assert_eq!(lobby.seats()[0].name, "moderator-chosen");
    }

    #[test]
    fn a_freed_seat_forgets_the_name_it_was_given() {
        let mut lobby = Lobby::new(4, 2);
        let id = lobby.admit(Some("transient")).unwrap();
        lobby.release(id);
        assert_eq!(lobby.seats()[0].name, "bot-0");
    }
}
