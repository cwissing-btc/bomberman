//! Keyboard to action.
//!
//! The most recently pressed direction wins while several are held, which is
//! how players expect a grid game to feel: tap Left while holding Up and you
//! turn left, let go of Left and you carry on up.

use bomber_domain::shared::Direction;
use bomber_protocol::Action;

#[derive(Debug, Default)]
pub struct Steering {
    held: Vec<Direction>,
    /// Ticks for which a bomb request is still repeated. One press becomes a
    /// few packets, so a single lost datagram does not swallow the bomb; the
    /// server ignores a second bomb on the same cell and actions mid-step.
    bomb_ticks: u8,
}

pub const BOMB_REPEAT_TICKS: u8 = 3;

impl Steering {
    pub fn press(&mut self, dir: Direction) {
        self.held.retain(|d| *d != dir);
        self.held.push(dir);
    }

    pub fn release(&mut self, dir: Direction) {
        self.held.retain(|d| *d != dir);
    }

    pub fn bomb(&mut self) {
        self.bomb_ticks = BOMB_REPEAT_TICKS;
    }

    pub fn clear(&mut self) {
        self.held.clear();
        self.bomb_ticks = 0;
    }

    /// The action for this tick. Call once per game tick.
    pub fn action(&mut self) -> Action {
        let bomb = self.bomb_ticks > 0;
        self.bomb_ticks = self.bomb_ticks.saturating_sub(1);
        match (self.held.last(), bomb) {
            (Some(&dir), bomb) => Action::moving(dir, bomb),
            (None, true) => Action::Bomb,
            (None, false) => Action::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_direction_wins_and_release_falls_back() {
        let mut s = Steering::default();
        s.press(Direction::Up);
        s.press(Direction::Left);
        assert_eq!(s.action(), Action::Left);
        s.release(Direction::Left);
        assert_eq!(s.action(), Action::Up);
        s.release(Direction::Up);
        assert_eq!(s.action(), Action::Idle);
    }

    #[test]
    fn a_bomb_combines_with_the_held_direction_for_a_few_ticks() {
        let mut s = Steering::default();
        s.press(Direction::Right);
        s.bomb();
        for _ in 0..BOMB_REPEAT_TICKS {
            assert_eq!(s.action(), Action::RightBomb);
        }
        assert_eq!(s.action(), Action::Right);
    }
}
