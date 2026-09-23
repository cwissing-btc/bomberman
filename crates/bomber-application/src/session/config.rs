use bomber_domain::board::{GenerationConfig, Symmetry};
use bomber_domain::game::Rules;

use crate::moderation::MapSettingsPatch;

/// Map settings for the *next* match.
///
/// Separate from [`GenerationConfig`] because it carries the seed policy, which
/// the generator itself has no opinion about.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MapSettings {
    pub generation: GenerationConfig,
    /// `0` -- the default -- means "roll a fresh seed for every match". Any
    /// other value pins the board, which is what a tournament or a bug report
    /// wants.
    pub seed: u64,
}

impl MapSettings {
    pub fn apply(&mut self, patch: MapSettingsPatch) {
        if let Some(width) = patch.width {
            self.generation.width = width;
        }
        if let Some(height) = patch.height {
            self.generation.height = height;
        }
        if let Some(density) = patch.density {
            self.generation.soft_block_density = density.clamp(0.0, 1.0);
        }
        if let Some(symmetry) = patch.symmetry {
            self.generation.symmetry = symmetry;
        }
        if let Some(seed) = patch.seed {
            self.seed = seed;
        }
    }

    pub fn symmetry(&self) -> Symmetry {
        self.generation.symmetry
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionConfig {
    pub rules: Rules,
    pub map: MapSettings,
    pub min_players: u8,
    pub max_players: u8,
    /// How often a full keyframe goes out. This is the ceiling on how long a
    /// bot can stay desynced after losing a datagram, since it has no way to
    /// ask for a resend.
    pub keyframe_interval_ticks: u32,
    /// Length of the pre-match countdown.
    pub countdown_ticks: u16,
    /// How many consecutive ticks `MATCH_INIT` is repeated on. Repetition is
    /// the only delivery guarantee available without acknowledgements.
    pub match_init_repeats: u32,
    /// How often lobby heartbeats go out while waiting.
    pub lobby_status_interval_ticks: u32,
    /// A seat with no packet for this long is shown as stale.
    pub stale_after_ticks: u32,
    /// A player silent for this long loses their seat. `0` never drops anyone.
    pub drop_after_ticks: u32,
    /// Whether seated players may start, pause, resume and abort matches
    /// themselves via the lobby request codes of the uplink.
    pub player_control: bool,
    /// Seats to fill with a server bot from the start, bit `i` for seat `i`.
    /// Only takes effect when the session has bots to seat, see
    /// [`crate::ArenaSession::with_bots`].
    pub bot_seats: u8,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            rules: Rules::default(),
            map: MapSettings::default(),
            min_players: 2,
            max_players: 4,
            keyframe_interval_ticks: 30,
            countdown_ticks: 180,
            match_init_repeats: 5,
            lobby_status_interval_ticks: 30,
            stale_after_ticks: 120,
            drop_after_ticks: 30 * 60,
            player_control: true,
            bot_seats: 0,
        }
    }
}
