use bomber_domain::shared::PlayerId;

use crate::codec::{ProtoError, Result};

use super::{Action, ClientPacket};

/// A lobby request to fill one seat with a server-side bot, or to stop doing so.
///
/// ```text
/// byte 0:  player_id                       the sender, as in every packet
/// byte 1:  (seq << 4) | 14
/// byte 2:  seat                            0..=3, the seat to configure
/// byte 3:  1 = fill with a bot, 0 = do not
/// ```
///
/// Like the hello, this is an exception to the two-byte rule that is sent a
/// handful of times, not sixty times a second. The first two bytes are an
/// ordinary action packet, so it is sequenced and authenticated exactly like
/// Start or Pause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatBotRequest {
    /// Who asks.
    pub player: PlayerId,
    pub seq: u8,
    /// Which seat to configure.
    pub seat: PlayerId,
    pub enabled: bool,
}

impl SeatBotRequest {
    pub const LEN: usize = 4;

    pub fn encode(&self) -> [u8; Self::LEN] {
        let [id, action] = ClientPacket::new(self.player, self.seq, Action::SeatBot).encode();
        [id, action, self.seat.raw(), self.enabled as u8]
    }

    /// Read the request. The caller has already decoded bytes 0 and 1 as a
    /// [`ClientPacket`] carrying [`Action::SeatBot`].
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let packet = ClientPacket::decode(buf)?;
        if packet.action != Action::SeatBot {
            return Err(ProtoError::invalid("seat_bot_action", packet.action.code()));
        }
        if buf.len() < Self::LEN {
            return Err(ProtoError::Truncated {
                at: buf.len(),
                need: Self::LEN - buf.len(),
            });
        }
        let enabled = match buf[3] {
            0 => false,
            1 => true,
            other => return Err(ProtoError::invalid("seat_bot_enabled", other)),
        };
        Ok(SeatBotRequest {
            player: PlayerId::new(packet.player_id),
            seq: packet.seq,
            seat: PlayerId::new(buf[2]),
            enabled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips() {
        for enabled in [false, true] {
            let request = SeatBotRequest {
                player: PlayerId::new(1),
                seq: 9,
                seat: PlayerId::new(3),
                enabled,
            };
            assert_eq!(SeatBotRequest::decode(&request.encode()), Ok(request));
        }
    }

    #[test]
    fn the_layout_is_the_documented_one() {
        let request = SeatBotRequest {
            player: PlayerId::new(2),
            seq: 5,
            seat: PlayerId::new(0),
            enabled: true,
        };
        assert_eq!(request.encode(), [2, 0x5E, 0, 1]);
    }

    #[test]
    fn a_request_without_its_payload_is_truncation() {
        assert!(matches!(
            SeatBotRequest::decode(&[0, 0x1E]),
            Err(ProtoError::Truncated { .. })
        ));
        assert!(matches!(
            SeatBotRequest::decode(&[0, 0x1E, 2]),
            Err(ProtoError::Truncated { .. })
        ));
    }

    #[test]
    fn another_action_or_a_bad_flag_is_refused() {
        assert!(SeatBotRequest::decode(&[0, 0x1A, 2, 1]).is_err());
        assert!(SeatBotRequest::decode(&[0, 0x1E, 2, 7]).is_err());
    }
}
