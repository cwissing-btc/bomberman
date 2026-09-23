use bomber_domain::shared::PlayerId;
use bomber_protocol::{Action, ServerFrame};

/// One seat played by the server itself.
///
/// It sees exactly what a networked client in that seat would receive and
/// answers with an ordinary action, so a server bot has no advantage over a
/// player -- and the session needs no second code path for it.
pub trait SeatBot: Send {
    /// A frame addressed to this seat, unicast or broadcast.
    fn observe(&mut self, frame: &ServerFrame);
    /// The action for the coming tick. [`Action::Idle`] sends nothing.
    fn act(&mut self, session_tick: u32) -> Action;
}

/// Where server bots come from.
///
/// A port rather than a dependency on the bot crate, so this layer stays free
/// of any particular bot and tests can seat a scripted one.
pub trait BotFactory: Send + Sync {
    fn create(&self, seat: PlayerId) -> Box<dyn SeatBot>;
}
