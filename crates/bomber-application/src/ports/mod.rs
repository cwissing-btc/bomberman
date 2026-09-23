//! The ports this layer is driven through.
//!
//! Kept small on purpose. The session is a synchronous state machine that
//! *returns* what should be sent rather than reaching out to send it, so only
//! two things genuinely need to be ports: entropy, and the bots the server
//! seats itself -- everything else is a return value, which is easier to test
//! than a mock.

mod bots;
mod entropy;

pub use bots::{BotFactory, SeatBot};
pub use entropy::{FixedEntropy, Entropy};
