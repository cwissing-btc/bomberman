use serde::{Deserialize, Serialize};

use bomber_domain::lobby::LobbyState;

use crate::codec::{ProtoError, Reader, Result, Writer};
use crate::uplink::Hello;

/// A heartbeat while waiting, so a bot can tell "nothing has started yet" from
/// "I have lost the server".
///
/// ```text
/// offset 5   u8   lobby state
///        6   u8   players connected
///        7   u8   max players
///        8   u8   slot mask
///        9   u16  countdown ticks
/// ---- 11 bytes: everything a bot written against protocol v1 reads ----
///       11   u8   flags: bit 0 paused, bit 1 can_start, bit 2 player_control,
///                 bit 3 server_bots
///       12   u8   min players
///       13   u8   seat count N
///       14   N x  { u8 seat flags (bit 0 occupied, bit 1 stale, bit 2 bot,
///                                  bit 3 bot_fill),
///                   u8 name length, name bytes (UTF-8, <= 24) }
/// ```
///
/// The tail is appended rather than inserted, so a bot that reads the first
/// eleven bytes and ignores the rest keeps working unchanged. It is what lets a
/// player's own client draw a proper lobby -- names, who has gone quiet,
/// whether Start would succeed -- without a second connection to the web port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyStatus {
    pub tick: u32,
    pub state: LobbyState,
    pub players_connected: u8,
    pub max_players: u8,
    /// Bit `i` set means seat `i` is taken.
    pub slot_mask: u8,
    /// Ticks left in the start countdown; 0 outside `Countdown`.
    pub countdown_ticks: u16,
    /// The appended tail. `None` only when decoding a frame from a server that
    /// predates it.
    pub details: Option<LobbyDetails>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyDetails {
    pub paused: bool,
    /// The server's own answer to "would Start succeed right now".
    pub can_start: bool,
    /// Whether seated players may send Start / Pause / Resume / Abort.
    pub player_control: bool,
    /// Whether this server can fill seats with its own bots.
    pub server_bots: bool,
    pub min_players: u8,
    pub seats: Vec<SeatStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatStatus {
    pub occupied: bool,
    /// The seat has not sent anything for longer than the server's threshold.
    pub stale: bool,
    /// The seat is held by one of the server's own bots.
    pub bot: bool,
    /// The seat is configured to be filled by a server bot whenever no player
    /// holds it. Set together with `occupied` and not `bot`, a player sits in
    /// it and the bot waits for them to leave.
    pub bot_fill: bool,
    pub name: String,
}

const FLAG_PAUSED: u8 = 0b001;
const FLAG_CAN_START: u8 = 0b010;
const FLAG_PLAYER_CONTROL: u8 = 0b100;
const FLAG_SERVER_BOTS: u8 = 0b1000;

const SEAT_OCCUPIED: u8 = 0b0001;
const SEAT_STALE: u8 = 0b0010;
const SEAT_BOT: u8 = 0b0100;
const SEAT_BOT_FILL: u8 = 0b1000;

impl LobbyStatus {
    pub fn encode(&self, w: &mut Writer) {
        w.u8(self.state.code())
            .u8(self.players_connected)
            .u8(self.max_players)
            .u8(self.slot_mask)
            .u16(self.countdown_ticks);

        let Some(details) = &self.details else {
            return;
        };
        let flags = (details.paused as u8 * FLAG_PAUSED)
            | (details.can_start as u8 * FLAG_CAN_START)
            | (details.player_control as u8 * FLAG_PLAYER_CONTROL)
            | (details.server_bots as u8 * FLAG_SERVER_BOTS);
        w.u8(flags)
            .u8(details.min_players)
            .u8(details.seats.len() as u8);
        for seat in &details.seats {
            let seat_flags = (seat.occupied as u8 * SEAT_OCCUPIED)
                | (seat.stale as u8 * SEAT_STALE)
                | (seat.bot as u8 * SEAT_BOT)
                | (seat.bot_fill as u8 * SEAT_BOT_FILL);
            let name = truncate(&seat.name, Hello::MAX_NAME_BYTES);
            w.u8(seat_flags).u8(name.len() as u8).bytes(name.as_bytes());
        }
    }

    pub fn decode(tick: u32, r: &mut Reader) -> Result<Self> {
        let raw = r.u8()?;
        let state =
            LobbyState::from_code(raw).ok_or_else(|| ProtoError::invalid("lobby_state", raw))?;
        let players_connected = r.u8()?;
        let max_players = r.u8()?;
        let slot_mask = r.u8()?;
        let countdown_ticks = r.u16()?;

        // A frame that ends here comes from a server without the tail. That is
        // a valid frame, not a truncated one.
        let details = if r.remaining() == 0 {
            None
        } else {
            Some(decode_details(r)?)
        };

        Ok(LobbyStatus {
            tick,
            state,
            players_connected,
            max_players,
            slot_mask,
            countdown_ticks,
            details,
        })
    }
}

fn decode_details(r: &mut Reader) -> Result<LobbyDetails> {
    let flags = r.u8()?;
    let min_players = r.u8()?;
    let count = r.u8()? as usize;
    let seats = r.repeat(count, |r| {
        let seat_flags = r.u8()?;
        let len = r.u8()? as usize;
        let raw = r.bytes(len)?;
        // The server sanitises names, so invalid UTF-8 here means corruption
        // in flight; show what survives rather than drop the whole lobby.
        let name = String::from_utf8_lossy(raw).into_owned();
        Ok(SeatStatus {
            occupied: seat_flags & SEAT_OCCUPIED != 0,
            stale: seat_flags & SEAT_STALE != 0,
            bot: seat_flags & SEAT_BOT != 0,
            bot_fill: seat_flags & SEAT_BOT_FILL != 0,
            name,
        })
    })?;
    Ok(LobbyDetails {
        paused: flags & FLAG_PAUSED != 0,
        can_start: flags & FLAG_CAN_START != 0,
        player_control: flags & FLAG_PLAYER_CONTROL != 0,
        server_bots: flags & FLAG_SERVER_BOTS != 0,
        min_players,
        seats,
    })
}

fn truncate(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> LobbyStatus {
        LobbyStatus {
            tick: 7,
            state: LobbyState::Open,
            players_connected: 2,
            max_players: 4,
            slot_mask: 0b0011,
            countdown_ticks: 0,
            details: Some(LobbyDetails {
                paused: false,
                can_start: true,
                player_control: true,
                server_bots: true,
                min_players: 2,
                seats: vec![
                    SeatStatus {
                        occupied: true,
                        stale: false,
                        bot: false,
                        bot_fill: true,
                        name: "Anna".into(),
                    },
                    SeatStatus {
                        occupied: true,
                        stale: true,
                        bot: false,
                        bot_fill: false,
                        name: "Bärbel 🙂".into(),
                    },
                    SeatStatus {
                        occupied: true,
                        stale: false,
                        bot: true,
                        bot_fill: true,
                        name: "Server-Bot 3".into(),
                    },
                    SeatStatus {
                        occupied: false,
                        stale: false,
                        bot: false,
                        bot_fill: false,
                        name: "bot-3".into(),
                    },
                ],
            }),
        }
    }

    fn round_trip(status: &LobbyStatus) -> LobbyStatus {
        let mut w = Writer::with_capacity(64);
        status.encode(&mut w);
        let bytes = w.finish();
        LobbyStatus::decode(status.tick, &mut Reader::new(&bytes)).unwrap()
    }

    #[test]
    fn the_extended_status_round_trips() {
        let status = sample();
        assert_eq!(round_trip(&status), status);
    }

    /// Old servers send eleven bytes; that must still decode.
    #[test]
    fn a_status_without_the_tail_is_still_valid() {
        let status = LobbyStatus {
            details: None,
            ..sample()
        };
        assert_eq!(round_trip(&status), status);
    }

    /// The first six payload bytes are exactly the v1 layout, so old bots are
    /// unaffected by the tail.
    #[test]
    fn the_v1_prefix_is_unchanged() {
        let mut w = Writer::with_capacity(64);
        sample().encode(&mut w);
        let bytes = w.finish();
        assert_eq!(&bytes[..6], &[0, 2, 4, 0b0011, 0, 0]);
    }
}
