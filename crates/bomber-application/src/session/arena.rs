use bomber_domain::board::{generate, Board};
use bomber_domain::game::{EndReason, Event, GameState, MatchOutcome};
use bomber_domain::lobby::{AdmissionError, Lobby, LobbyState};
use bomber_domain::shared::PlayerId;
use bomber_protocol::{
    Action, Assigned, LobbyDetails, LobbyStatus, MatchEnd, MatchInit, SeatStatus, ServerFrame,
    MAX_PLAYERS, PROTOCOL_VERSION, TICK_RATE,
};
use rand::{Rng, SeedableRng};
use rand_pcg::Pcg64Mcg;

use crate::moderation::{CommandOutcome, ModeratorCommand};
use crate::ports::{BotFactory, SeatBot};

use super::delivery::Delivery;
use super::inputs::{InputSlots, Rejection};
use super::presence::Presence;
use super::snapshot::{SeatView, SessionSnapshot};
use super::{SessionConfig, MapSettings};

/// What one tick produced, for the adapters to deliver.
///
/// The session never sends anything itself. It returns what *should* be sent
/// and lets the caller decide how -- which is why a whole match can be played
/// out in a unit test without a socket in sight.
#[derive(Debug, Default, Clone)]
pub struct TickReport {
    /// Frames addressed to one bot, because their content differs per player.
    pub unicast: Vec<(PlayerId, ServerFrame)>,
    /// Frames every connected bot gets verbatim.
    pub broadcast: Vec<ServerFrame>,
    /// Domain events from this tick, for the spectator feed.
    pub events: Vec<Event>,
    pub match_started: bool,
    pub match_ended: Option<MatchOutcome>,
    /// The lobby roster or state changed and watchers should be refreshed.
    pub lobby_changed: bool,
    /// Whether the simulation actually advanced.
    pub simulated: bool,
    /// Players dropped this tick for having been silent too long. The
    /// adapter forgets their transport binding, as for a kick.
    pub released: Vec<PlayerId>,
}

pub struct ArenaSession {
    config: SessionConfig,
    lobby: Lobby,
    game: Option<GameState>,
    board: Option<Board>,
    inputs: InputSlots,
    presence: Presence,
    delivery: Delivery,
    paused: bool,
    session_tick: u32,
    match_id: u32,
    match_init: Option<MatchInit>,
    init_repeats_left: u32,
    last_outcome: Option<MatchOutcome>,
    seeds: Pcg64Mcg,
    lobby_dirty: bool,
    /// An aborted match whose `MATCH_END` has not gone out yet. Commands run
    /// outside the tick, so the frame is emitted on the next one.
    pending_end: Option<MatchOutcome>,
    /// Ticks spent paused, which drives the lobby heartbeat while the match
    /// clock is frozen. Without it a paused client hears nothing and decides
    /// the server is gone.
    paused_ticks: u32,
    /// Where server bots come from. `None` on a server without them.
    bot_factory: Option<Box<dyn BotFactory>>,
    /// The bot playing each seat, indexed by seat.
    bots: Vec<Option<ServerBot>>,
}

/// A seat the server plays itself.
struct ServerBot {
    brain: Box<dyn SeatBot>,
    /// Its own sequence counter, so its moves go through the same duplicate
    /// filter as everyone else's.
    seq: u8,
}

impl ArenaSession {
    pub fn new(config: SessionConfig, entropy_seed: u64) -> Self {
        let seats = config.max_players.min(MAX_PLAYERS as u8);
        ArenaSession {
            lobby: Lobby::new(seats, config.min_players),
            inputs: InputSlots::with_seats(seats as usize),
            presence: Presence::with_seats(seats as usize),
            delivery: Delivery::new(config.keyframe_interval_ticks),
            config,
            game: None,
            board: None,
            paused: false,
            session_tick: 0,
            match_id: 0,
            match_init: None,
            init_repeats_left: 0,
            last_outcome: None,
            seeds: Pcg64Mcg::seed_from_u64(entropy_seed),
            lobby_dirty: true,
            pending_end: None,
            paused_ticks: 0,
            bot_factory: None,
            bots: (0..seats).map(|_| None).collect(),
        }
    }

    /// Let the session fill seats with bots of its own, starting with the
    /// seats named in [`SessionConfig::bot_seats`].
    pub fn with_bots(mut self, factory: Box<dyn BotFactory>) -> Self {
        self.bot_factory = Some(factory);
        for seat in 0..self.lobby.max_players() {
            if self.config.bot_seats & (1 << seat) != 0 {
                self.lobby.set_bot_fill(PlayerId::new(seat), true);
            }
        }
        self.sync_bots();
        self
    }

    pub fn server_bots(&self) -> bool {
        self.bot_factory.is_some()
    }

    // -- queries -------------------------------------------------------------

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn game(&self) -> Option<&GameState> {
        self.game.as_ref()
    }

    pub fn board(&self) -> Option<&Board> {
        self.board.as_ref()
    }

    pub fn state(&self) -> LobbyState {
        self.lobby.state()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn session_tick(&self) -> u32 {
        self.session_tick
    }

    pub fn match_id(&self) -> u32 {
        self.match_id
    }

    pub fn last_outcome(&self) -> Option<&MatchOutcome> {
        self.last_outcome.as_ref()
    }

    pub fn is_seated(&self, player: PlayerId) -> bool {
        self.lobby.is_occupied(player)
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let now = self.session_tick;
        let seats = self
            .lobby
            .seats()
            .iter()
            .map(|seat| {
                let stale_ticks = self.presence.staleness(seat.id, now);
                SeatView {
                    id: seat.id,
                    name: seat.name.clone(),
                    connected: seat.occupied,
                    bot: seat.bot,
                    bot_fill: seat.bot_fill,
                    presence: self.presence.of(seat.id, self.inputs.loss_pct(seat.id)),
                    stale_ticks,
                    stale: self.is_stale(seat, stale_ticks),
                }
            })
            .collect();

        SessionSnapshot {
            state: self.lobby.state(),
            paused: self.paused,
            can_start: !self.paused && self.lobby.can_start(),
            server_bots: self.server_bots(),
            min_players: self.lobby.min_players(),
            max_players: self.lobby.max_players(),
            countdown_ticks: self.lobby.countdown_ticks(),
            map: self.config.map,
            seats,
            session_tick: now,
            board: self.board.clone(),
        }
    }

    /// A seat that has gone quiet. A server bot never is: it has no link to
    /// lose, and it sends nothing while it has nothing to do.
    fn is_stale(&self, seat: &bomber_domain::lobby::Seat, stale_ticks: Option<u32>) -> bool {
        seat.occupied
            && !seat.bot
            && stale_ticks.is_none_or(|t| t > self.config.stale_after_ticks)
    }

    // -- admission -----------------------------------------------------------

    /// Seat a new bot, optionally under the name it proposed when joining.
    ///
    /// The caller binds the returned id to whatever transport the hello arrived
    /// on, and is responsible for rejecting later packets that claim the id
    /// from anywhere else.
    pub fn admit(&mut self, proposed_name: Option<&str>) -> Result<PlayerId, AdmissionError> {
        let player = self.lobby.admit(proposed_name)?;
        self.inputs.reset_seat(player);
        self.presence.reset_seat(player);
        // The hello counts as a sign of life, so the timeout for silent
        // players runs from the moment of joining.
        self.presence.touch(player, self.session_tick);
        self.lobby_dirty = true;
        // The player may have taken a bot's seat.
        self.sync_bots();
        Ok(player)
    }

    /// A seated player said hello again. That is a sign of life like any
    /// other packet.
    pub fn heard_from(&mut self, player: PlayerId) {
        if self.lobby.is_occupied(player) && !self.lobby.is_bot(player) {
            self.presence.touch(player, self.session_tick);
        }
    }

    /// Free a seat. If it is set to be filled by a bot, the bot sits down as
    /// soon as the lobby allows it.
    pub fn release(&mut self, player: PlayerId) {
        self.lobby.release(player);
        self.inputs.reset_seat(player);
        self.presence.reset_seat(player);
        self.lobby_dirty = true;
        self.sync_bots();
    }

    /// Bring the bots in line with the seat settings: seat one wherever a seat
    /// asks for it and stands empty, drop the ones whose seat no longer wants
    /// them or was taken by a player.
    ///
    /// Bots only sit down between matches. A player kicked mid-match leaves a
    /// hole until the results are up, exactly as without bots.
    ///
    /// Bots keep players company; they do not play among themselves. Once the
    /// last player is gone every bot leaves, the seats fall back to the
    /// configured [`SessionConfig::bot_seats`] for whoever comes next, and a
    /// match the bots were still playing is aborted.
    fn sync_bots(&mut self) {
        let players = self
            .lobby
            .seats()
            .iter()
            .filter(|s| s.occupied && !s.bot)
            .count();
        if players == 0 {
            for seat in 0..self.lobby.max_players() {
                let id = PlayerId::new(seat);
                let default = self.config.bot_seats & (1 << seat) != 0;
                if self.lobby.seats()[seat as usize].bot_fill != default {
                    self.lobby.set_bot_fill(id, default);
                    self.lobby_dirty = true;
                }
            }
            let bots_seated = self.lobby.seats().iter().any(|s| s.bot);
            if bots_seated {
                match self.lobby.state() {
                    LobbyState::Running => {
                        let _ = self.end_match();
                    }
                    LobbyState::Countdown => {
                        let _ = self.reset();
                    }
                    _ => {}
                }
            }
        }

        let may_seat = self.bot_factory.is_some()
            && players > 0
            && !matches!(
                self.lobby.state(),
                LobbyState::Countdown | LobbyState::Running
            );
        for index in 0..self.lobby.seats().len() {
            let seat = &self.lobby.seats()[index];
            let id = seat.id;
            if seat.bot && (!seat.bot_fill || players == 0) {
                self.lobby.release(id);
                self.inputs.reset_seat(id);
                self.presence.reset_seat(id);
                self.lobby_dirty = true;
            } else if seat.bot_fill && !seat.occupied && may_seat && self.lobby.seat_bot(id) {
                self.inputs.reset_seat(id);
                self.presence.reset_seat(id);
                self.lobby_dirty = true;
            }

            let wanted = self.lobby.is_bot(id);
            let slot = &mut self.bots[index];
            match (wanted, slot.is_some(), &self.bot_factory) {
                (true, false, Some(factory)) => {
                    *slot = Some(ServerBot {
                        brain: factory.create(id),
                        seq: 0,
                    });
                }
                (false, true, _) => *slot = None,
                _ => {}
            }
        }
    }

    /// Record a bot's intent for this tick.
    ///
    /// A lobby request is routed to [`ArenaSession::player_request`]; call
    /// that directly to learn what became of it.
    pub fn submit(&mut self, player: PlayerId, action: Action, seq: u8) -> Result<(), Rejection> {
        if action.is_lobby_request() {
            return self.player_request(player, action, seq).map(|_| ());
        }
        if !self.lobby.is_occupied(player) {
            return Err(Rejection::UnknownSeat);
        }
        self.presence.record(player, self.session_tick);
        self.inputs.submit(player, action, seq)
    }

    /// A seated player asks to start, pause, resume or abort the match.
    ///
    /// The caller has already bound `player` to the address the packet came
    /// from, so this is as trustworthy as a move. A duplicated datagram is
    /// rejected as stale by the sequence counter and never runs twice.
    ///
    /// Players get a narrower set of powers than the moderation socket: no
    /// kicking, renaming, locking or map changes, and pause only while a match
    /// is actually on.
    pub fn player_request(
        &mut self,
        player: PlayerId,
        action: Action,
        seq: u8,
    ) -> Result<CommandOutcome, Rejection> {
        if !self.lobby.is_occupied(player) {
            return Err(Rejection::UnknownSeat);
        }
        self.presence.record(player, self.session_tick);
        self.inputs.note(player, seq)?;

        if !self.config.player_control {
            return Ok(CommandOutcome::rejected(
                "player control is disabled on this server",
            ));
        }

        let outcome = match action {
            Action::Start => self.player_start(),
            Action::Pause if !self.lobby.state().is_playing() => {
                CommandOutcome::rejected("nothing to pause outside a match")
            }
            Action::Pause => self.execute(ModeratorCommand::Pause),
            Action::Resume => self.execute(ModeratorCommand::Resume),
            Action::Abort => self.execute(ModeratorCommand::End),
            Action::SeatBot => CommandOutcome::rejected("a seat-bot request needs its seat"),
            _ => CommandOutcome::rejected("not a lobby request"),
        };
        Ok(outcome)
    }

    /// A seated player asks for a seat to be filled by a server bot, or for
    /// the bot to go.
    ///
    /// Authenticated and deduplicated like every other lobby request. A player
    /// may do this in the open lobby and on the results screen; a locked lobby
    /// is the moderator's to fill.
    pub fn seat_bot_request(
        &mut self,
        player: PlayerId,
        seat: PlayerId,
        enabled: bool,
        seq: u8,
    ) -> Result<CommandOutcome, Rejection> {
        if !self.lobby.is_occupied(player) {
            return Err(Rejection::UnknownSeat);
        }
        self.presence.record(player, self.session_tick);
        self.inputs.note(player, seq)?;

        if !self.config.player_control {
            return Ok(CommandOutcome::rejected(
                "player control is disabled on this server",
            ));
        }
        if !self.lobby.state().admits_new_players() {
            return Ok(CommandOutcome::rejected(
                "bots can only be changed in the open lobby or after a match",
            ));
        }
        Ok(self.execute(ModeratorCommand::SeatBot {
            player: seat,
            enabled,
        }))
    }

    /// Start from the lobby, or -- after a match -- clear the results and
    /// start the next one in a single step.
    ///
    /// A moderator has separate `reset` and `start` so results can be studied
    /// before they are dismissed. A player pressing Start on the results
    /// screen has done exactly that, so here the two are one gesture. The
    /// player count is checked first, so a refused start leaves the results
    /// where they are.
    fn player_start(&mut self) -> CommandOutcome {
        if self.lobby.state() == LobbyState::MatchOver {
            let have = self.lobby.occupied_count();
            let need = self.lobby.min_players();
            if have < need {
                return CommandOutcome::rejected(format!(
                    "need at least {need} players, have {have}"
                ));
            }
            self.execute(ModeratorCommand::Reset);
        }
        self.execute(ModeratorCommand::Start)
    }

    // -- frames the adapter sends outside the tick ---------------------------

    pub fn assigned_frame(&self, player: PlayerId) -> ServerFrame {
        ServerFrame::Assigned(Assigned {
            tick: self.session_tick,
            protocol_version: PROTOCOL_VERSION,
            player_id: player,
            tick_rate: TICK_RATE,
            max_players: self.lobby.max_players(),
        })
    }

    /// The match-start frame for one player, if a match is under way.
    ///
    /// Re-sending this is the documented recovery path for a bot that missed
    /// it: with no acknowledgements, asking again is the only option it has.
    pub fn match_init_frame_for(&self, player: PlayerId) -> Option<ServerFrame> {
        let init = self.match_init.as_ref()?;
        Some(ServerFrame::MatchInit(MatchInit {
            tick: self.session_tick,
            your_player_id: player,
            ..init.clone()
        }))
    }

    pub fn lobby_status_frame(&self) -> ServerFrame {
        let now = self.session_tick;
        let seats = self
            .lobby
            .seats()
            .iter()
            .map(|seat| SeatStatus {
                occupied: seat.occupied,
                stale: self.is_stale(seat, self.presence.staleness(seat.id, now)),
                bot: seat.bot,
                bot_fill: seat.bot_fill,
                name: seat.name.clone(),
            })
            .collect();

        ServerFrame::LobbyStatus(LobbyStatus {
            tick: now,
            state: self.lobby.state(),
            players_connected: self.lobby.occupied_count(),
            max_players: self.lobby.max_players(),
            slot_mask: self.lobby.occupancy_mask(),
            countdown_ticks: self.lobby.countdown_ticks(),
            details: Some(LobbyDetails {
                paused: self.paused,
                can_start: !self.paused && self.lobby.can_start(),
                player_control: self.config.player_control,
                server_bots: self.server_bots(),
                min_players: self.lobby.min_players(),
                seats,
            }),
        })
    }

    pub fn take_lobby_dirty(&mut self) -> bool {
        std::mem::take(&mut self.lobby_dirty)
    }

    // -- the tick ------------------------------------------------------------

    pub fn tick(&mut self) -> TickReport {
        let report = self.advance();
        self.show_bots(&report);
        report
    }

    fn advance(&mut self) -> TickReport {
        let mut report = TickReport::default();
        if self.paused {
            // The match clock is frozen, but the wall clock is not: keep
            // telling the bots the server is alive and paused.
            self.paused_ticks = self.paused_ticks.wrapping_add(1);
            let changed = self.take_lobby_dirty();
            if changed
                || self
                    .paused_ticks
                    .is_multiple_of(self.config.lobby_status_interval_ticks.max(1))
            {
                report.broadcast.push(self.lobby_status_frame());
            }
            report.lobby_changed = changed;
            return report;
        }
        self.paused_ticks = 0;

        self.session_tick += 1;
        self.presence.advance(self.session_tick);
        self.drop_silent_players(&mut report);
        // Seats freed by the end of a match, or the lobby reopening, are
        // filled here, before this tick's lobby status is built.
        self.sync_bots();

        if let Some(outcome) = self.pending_end.take() {
            let tick = self.game.as_ref().map_or(self.session_tick, |g| g.tick);
            report
                .broadcast
                .push(ServerFrame::MatchEnd(MatchEnd::from_outcome(tick, &outcome)));
            report.match_ended = Some(outcome);
        }

        match self.lobby.state() {
            LobbyState::Open | LobbyState::Locked | LobbyState::MatchOver => {
                self.heartbeat(&mut report, self.config.lobby_status_interval_ticks);
            }
            LobbyState::Countdown => {
                // Faster while counting down: the number on screen is changing
                // and a bot may want to get ready.
                self.heartbeat(&mut report, 10);
                if self.lobby.tick_countdown() {
                    self.begin_match(&mut report);
                }
            }
            LobbyState::Running => self.advance_match(&mut report),
        }

        report.lobby_changed |= self.take_lobby_dirty();
        // Whatever changed the lobby -- a join, a command, a match starting or
        // ending -- the players hear about it now rather than at the next
        // heartbeat, so a Start pressed on one client shows on all of them at
        // once. The periodic heartbeat above only runs while waiting.
        if report.lobby_changed
            && !report
                .broadcast
                .iter()
                .any(|f| matches!(f, ServerFrame::LobbyStatus(_)))
        {
            report.broadcast.push(self.lobby_status_frame());
        }
        report
    }

    /// Free the seats of players who have sent nothing for
    /// [`SessionConfig::drop_after_ticks`]. Their client has crashed or gone;
    /// if it comes back it says hello and is seated anew.
    ///
    /// Paused time does not count: the session clock stands still with the
    /// match, and a player waiting out a pause is not gone.
    fn drop_silent_players(&mut self, report: &mut TickReport) {
        let limit = self.config.drop_after_ticks;
        if limit == 0 {
            return;
        }
        let now = self.session_tick;
        let silent: Vec<PlayerId> = self
            .lobby
            .seats()
            .iter()
            .filter(|s| s.occupied && !s.bot)
            .filter(|s| self.presence.staleness(s.id, now).is_none_or(|t| t > limit))
            .map(|s| s.id)
            .collect();
        for player in silent {
            self.release(player);
            report.released.push(player);
        }
    }

    /// Hand every server bot what a client in its seat would have received
    /// from this tick, in the order the network adapter sends it.
    fn show_bots(&mut self, report: &TickReport) {
        for (index, slot) in self.bots.iter_mut().enumerate() {
            let Some(bot) = slot else {
                continue;
            };
            let seat = PlayerId::new(index as u8);
            for (to, frame) in &report.unicast {
                if *to == seat {
                    bot.brain.observe(frame);
                }
            }
            for frame in &report.broadcast {
                bot.brain.observe(frame);
            }
        }
    }

    /// Collect this tick's moves from the server bots.
    ///
    /// They act on the state of the previous tick -- the same view a client
    /// with no latency at all would have -- and like a client they send
    /// nothing when they have nothing to do.
    fn bot_moves(&mut self) {
        let now = self.session_tick;
        for (index, slot) in self.bots.iter_mut().enumerate() {
            let Some(bot) = slot else {
                continue;
            };
            let action = bot.brain.act(now);
            if action == Action::Idle || action.is_lobby_request() || action == Action::Hello {
                continue;
            }
            let seat = PlayerId::new(index as u8);
            self.presence.record(seat, now);
            let _ = self.inputs.submit(seat, action, bot.seq);
            bot.seq = (bot.seq + 1) & 0x0F;
        }
    }

    fn heartbeat(&self, report: &mut TickReport, interval: u32) {
        if self.session_tick.is_multiple_of(interval.max(1)) {
            report.broadcast.push(self.lobby_status_frame());
        }
    }

    fn begin_match(&mut self, report: &mut TickReport) {
        let participants = self.lobby.participants();
        if participants.is_empty() {
            self.lobby.reset();
            self.lobby_dirty = true;
            return;
        }

        let seed = match self.config.map.seed {
            0 => self.seeds.gen(),
            pinned => pinned,
        };
        let board = generate(
            &self.config.map.generation,
            seed,
            participants.len() as u8,
        );

        self.match_id = self.match_id.wrapping_add(1);
        self.match_init = Some(MatchInit {
            tick: self.session_tick,
            protocol_version: PROTOCOL_VERSION,
            match_id: self.match_id,
            seed,
            // Overwritten per recipient; this frame is never sent as-is.
            your_player_id: participants[0],
            grid: board.grid.clone(),
            spawns: board.spawns.clone(),
            rules: self.config.rules,
        });
        self.init_repeats_left = self.config.match_init_repeats;

        let game = GameState::new(board.clone(), self.config.rules, seed, &participants);
        self.board = Some(board);
        self.game = Some(game);
        self.inputs.clear();
        self.delivery.restart();
        self.lobby.begin_match();
        self.last_outcome = None;
        self.lobby_dirty = true;

        self.repeat_match_init(report);
        // Clients hold nothing yet, so the first frame has to be complete --
        // and it has to go through , so the keyframe cadence is
        // measured from the start of the match rather than from the first
        // simulated tick.
        let game = self.game.as_ref().expect("just created");
        let opening = self.delivery.frame_for(game, &[]);
        report.broadcast.push(opening);
        report.match_started = true;
    }

    fn repeat_match_init(&mut self, report: &mut TickReport) {
        if self.init_repeats_left == 0 {
            return;
        }
        self.init_repeats_left -= 1;
        for seat in self.lobby.seats() {
            if !seat.occupied {
                continue;
            }
            if let Some(frame) = self.match_init_frame_for(seat.id) {
                report.unicast.push((seat.id, frame));
            }
        }
    }

    fn advance_match(&mut self, report: &mut TickReport) {
        self.bot_moves();
        let intents = self.inputs.take();
        let Some(game) = self.game.as_mut() else {
            return;
        };
        let outcome = game.step(&intents);
        report.simulated = true;
        report.events = outcome.events;

        self.repeat_match_init(report);

        // Disjoint field borrows: `delivery` and `game` are separate fields.
        let game = self.game.as_ref().expect("checked above");
        let frame = self.delivery.frame_for(game, &report.events);
        let tick = game.tick;
        report.broadcast.push(frame);

        if let Some(result) = outcome.ended {
            report
                .broadcast
                .push(ServerFrame::MatchEnd(MatchEnd::from_outcome(tick, &result)));
            self.lobby.finish_match();
            self.last_outcome = Some(result.clone());
            report.match_ended = Some(result);
            self.lobby_dirty = true;
        }
    }

    // -- moderation ----------------------------------------------------------

    pub fn execute(&mut self, command: ModeratorCommand) -> CommandOutcome {
        let outcome = self.dispatch(command);
        if outcome.is_accepted() {
            self.lobby_dirty = true;
            self.sync_bots();
        }
        outcome
    }

    fn dispatch(&mut self, command: ModeratorCommand) -> CommandOutcome {
        match command {
            ModeratorCommand::Start => self.start(),
            ModeratorCommand::Pause => self.pause(),
            ModeratorCommand::Resume => self.resume(),
            ModeratorCommand::End => self.end_match(),
            ModeratorCommand::Reset => self.reset(),
            ModeratorCommand::Lock => {
                self.lobby.lock();
                CommandOutcome::Accepted
            }
            ModeratorCommand::Unlock => {
                self.lobby.unlock();
                CommandOutcome::Accepted
            }
            ModeratorCommand::Kick(player) => self.kick(player),
            ModeratorCommand::Rename { player, name } => {
                if !self.lobby.seats().iter().any(|s| s.id == player) {
                    return CommandOutcome::rejected(format!("no seat {player}"));
                }
                self.lobby.rename(player, &name);
                CommandOutcome::Accepted
            }
            ModeratorCommand::SeatBot { player, enabled } => self.set_seat_bot(player, enabled),
            ModeratorCommand::ConfigureMap(patch) => {
                if self.lobby.state().is_playing() {
                    return CommandOutcome::rejected(
                        "cannot change the map while a match is running",
                    );
                }
                self.config.map.apply(patch);
                CommandOutcome::Accepted
            }
            ModeratorCommand::PreviewMap => {
                let (board, seed) = self.preview();
                CommandOutcome::Preview {
                    board: Box::new(board),
                    seed,
                }
            }
        }
    }

    fn start(&mut self) -> CommandOutcome {
        if self.paused {
            return CommandOutcome::rejected("resume before starting");
        }
        match self.lobby.begin_countdown(self.config.countdown_ticks) {
            Ok(()) => CommandOutcome::Accepted,
            Err(bomber_domain::lobby::StartError::AlreadyRunning) => {
                CommandOutcome::rejected("a match is already starting or running")
            }
            Err(bomber_domain::lobby::StartError::NotEnoughPlayers { have, need }) => {
                CommandOutcome::rejected(format!("need at least {need} players, have {have}"))
            }
        }
    }

    fn pause(&mut self) -> CommandOutcome {
        if self.paused {
            return CommandOutcome::rejected("already paused");
        }
        self.paused = true;
        CommandOutcome::Accepted
    }

    fn resume(&mut self) -> CommandOutcome {
        if !self.paused {
            return CommandOutcome::rejected("not paused");
        }
        self.paused = false;
        CommandOutcome::Accepted
    }

    /// Stop the running match, leaving the results up.
    fn end_match(&mut self) -> CommandOutcome {
        if !matches!(
            self.lobby.state(),
            LobbyState::Running | LobbyState::Countdown
        ) {
            return CommandOutcome::rejected("no match to end");
        }
        if let Some(game) = self.game.as_mut() {
            game.finished = true;
            let outcome = game.outcome(EndReason::Aborted);
            self.last_outcome = Some(outcome.clone());
            // Bots and viewers learn of it through the same MATCH_END a
            // natural finish produces, instead of having to infer it.
            self.pending_end = Some(outcome);
        }
        self.lobby.finish_match();
        self.paused = false;
        CommandOutcome::Accepted
    }

    /// Clear the results and go back to accepting players.
    fn reset(&mut self) -> CommandOutcome {
        self.pending_end = None;
        self.game = None;
        self.match_init = None;
        self.init_repeats_left = 0;
        self.inputs.clear();
        self.delivery.restart();
        self.paused = false;
        self.lobby.reset();
        CommandOutcome::Accepted
    }

    fn set_seat_bot(&mut self, seat: PlayerId, enabled: bool) -> CommandOutcome {
        if self.bot_factory.is_none() {
            return CommandOutcome::rejected("this server has no bots of its own");
        }
        if !self.lobby.seats().iter().any(|s| s.id == seat) {
            return CommandOutcome::rejected(format!("no seat {seat}"));
        }
        if matches!(
            self.lobby.state(),
            LobbyState::Countdown | LobbyState::Running
        ) {
            return CommandOutcome::rejected(
                "cannot change bots while a match is starting or running",
            );
        }
        self.lobby.set_bot_fill(seat, enabled);
        // `execute` seats or removes the bot.
        CommandOutcome::Accepted
    }

    /// Free a seat. Kicking a server bot also stops the seat from being
    /// refilled, or it would be back on the next tick; kicking a player from a
    /// bot seat hands it to the bot.
    fn kick(&mut self, player: PlayerId) -> CommandOutcome {
        if !self.lobby.is_occupied(player) {
            return CommandOutcome::rejected(format!("seat {player} is already empty"));
        }
        if self.lobby.is_bot(player) {
            self.lobby.set_bot_fill(player, false);
        }
        self.release(player);
        CommandOutcome::AcceptedAndReleased(player)
    }

    /// Generate a board with the current settings without starting anything.
    fn preview(&mut self) -> (Board, u64) {
        let seed = match self.config.map.seed {
            0 => self.seeds.gen(),
            pinned => pinned,
        };
        let players = self.lobby.occupied_count().max(self.config.min_players);
        (generate(&self.config.map.generation, seed, players), seed)
    }

    /// The settings the next match will use.
    pub fn map_settings(&self) -> MapSettings {
        self.config.map
    }
}
