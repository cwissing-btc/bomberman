use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use bomber_application::CommandOutcome;
use bomber_domain::shared::PlayerId;
use bomber_protocol::{Action, ClientPacket, Hello, SeatBotRequest, ServerFrame, MAX_DATAGRAM};
use tokio::net::UdpSocket;

use crate::runtime::Shared;

/// The bot-facing socket.
#[derive(Clone)]
pub struct Endpoint {
    socket: Arc<UdpSocket>,
}

impl Endpoint {
    pub async fn bind(addr: SocketAddr) -> Result<Self> {
        let socket = UdpSocket::bind(addr).await?;
        tracing::info!(%addr, "listening for bots");
        Ok(Endpoint {
            socket: Arc::new(socket),
        })
    }

    pub async fn send(&self, addr: SocketAddr, frame: &ServerFrame) {
        let bytes = frame.encode();
        if bytes.len() > MAX_DATAGRAM {
            // Better a loud log than a datagram that silently never arrives.
            tracing::warn!(
                frame = frame.frame_type(),
                size = bytes.len(),
                "frame exceeds the datagram budget and may be dropped in flight"
            );
        }
        if let Err(error) = self.socket.send_to(&bytes, addr).await {
            tracing::debug!(%addr, %error, "send failed");
        }
    }

    pub async fn send_to_player(&self, shared: &Shared, player: PlayerId, frame: &ServerFrame) {
        let addr = shared.registry.lock().unwrap().addr_of(player);
        if let Some(addr) = addr {
            self.send(addr, frame).await;
        }
    }
}

/// Receive datagrams until the socket dies.
pub async fn listen(endpoint: Endpoint, shared: Shared) {
    let mut buf = [0u8; 64];
    loop {
        let (len, addr) = match endpoint.socket.recv_from(&mut buf).await {
            Ok(received) => received,
            Err(error) => {
                tracing::error!(%error, "udp receive failed");
                continue;
            }
        };

        // Anything can arrive on a UDP port. A packet that does not decode is
        // not worth a log line at info level -- it is the normal background
        // noise of an open port.
        let Ok(packet) = ClientPacket::decode(&buf[..len]) else {
            continue;
        };

        if packet.is_hello() {
            // A hello may carry a name. An unusable one costs the bot its name,
            // not its seat -- and it is logged, because the bot has no channel
            // to be told on.
            let name = match Hello::decode(&buf[..len]) {
                Ok(hello) => hello.name,
                Err(error) => {
                    tracing::debug!(%addr, %error, "hello carried an unreadable name");
                    None
                }
            };
            handle_hello(&endpoint, &shared, addr, name).await;
        } else {
            handle_action(&shared, addr, packet, &buf[..len]);
        }
    }
}

async fn handle_hello(
    endpoint: &Endpoint,
    shared: &Shared,
    addr: SocketAddr,
    proposed_name: Option<String>,
) {
    let existing = shared.registry.lock().unwrap().player_of(&addr);

    let (player, frames) = {
        let mut session = shared.session.lock().unwrap();
        let player = match existing {
            Some(player) => {
                session.heard_from(player);
                player
            }
            None => match session.admit(proposed_name.as_deref()) {
                Ok(player) => {
                    shared.registry.lock().unwrap().bind(addr, player);
                    tracing::info!(%addr, %player, name = ?proposed_name, "seated");
                    player
                }
                Err(reason) => {
                    // The bot is expected to retry, so this is routine.
                    tracing::debug!(%addr, ?reason, "refused");
                    return;
                }
            },
        };

        // Re-sending the match frame is the documented recovery path for a bot
        // that missed it: without acknowledgements, asking again is all it has.
        let mut frames = vec![session.assigned_frame(player)];
        frames.extend(session.match_init_frame_for(player));
        frames.push(session.lobby_status_frame());
        (player, frames)
    };

    for frame in &frames {
        endpoint.send(addr, frame).await;
    }
    shared.notify_lobby_changed();
    let _ = player;
}

fn handle_action(shared: &Shared, addr: SocketAddr, packet: ClientPacket, raw: &[u8]) {
    let Some(bound) = shared.registry.lock().unwrap().player_of(&addr) else {
        return;
    };
    // Byte 0 is a claim, not a credential. Believe it only from the address
    // that owns the seat.
    if packet.claimed_player() != Some(bound) {
        tracing::debug!(%addr, claimed = packet.player_id, %bound, "id does not match source");
        return;
    }

    if packet.action == Action::SeatBot {
        match SeatBotRequest::decode(raw) {
            Ok(request) => handle_seat_bot_request(shared, bound, request),
            Err(error) => tracing::debug!(%addr, %error, "unreadable seat-bot request"),
        }
        return;
    }
    if packet.action.is_lobby_request() {
        handle_lobby_request(shared, bound, packet);
        return;
    }

    let mut session = shared.session.lock().unwrap();
    let _ = session.submit(bound, packet.action, packet.seq);
}

/// A seated player switched server-bot filling for a seat in their lobby.
fn handle_seat_bot_request(shared: &Shared, player: PlayerId, request: SeatBotRequest) {
    let outcome = {
        let mut session = shared.session.lock().unwrap();
        session.seat_bot_request(player, request.seat, request.enabled, request.seq)
    };
    match outcome {
        Err(_) => {}
        Ok(CommandOutcome::Rejected(reason)) => {
            tracing::debug!(%player, seat = %request.seat, %reason, "seat-bot request refused");
        }
        Ok(_) => {
            tracing::info!(
                %player,
                seat = %request.seat,
                enabled = request.enabled,
                "seat-bot request accepted"
            );
            shared.notify_lobby_changed();
        }
    }
}

/// A seated player pressed Start, Pause, Resume or Abort in their client.
///
/// Logged at info because it changes the match for everyone, and when four
/// people share a lobby "who stopped it?" is a question somebody will ask.
fn handle_lobby_request(shared: &Shared, player: PlayerId, packet: ClientPacket) {
    let outcome = {
        let mut session = shared.session.lock().unwrap();
        session.player_request(player, packet.action, packet.seq)
    };
    match outcome {
        // A duplicate of a request already handled: expected, silent.
        Err(_) => {}
        Ok(CommandOutcome::Rejected(reason)) => {
            tracing::debug!(%player, action = ?packet.action, %reason, "player request refused");
        }
        Ok(_) => {
            tracing::info!(%player, action = ?packet.action, "player request accepted");
            // Web watchers see it immediately; bots hear it on the next tick.
            shared.notify_lobby_changed();
        }
    }
}
