//! The UDP link to the server.
//!
//! One socket for everything, because the server binds a seat to the source
//! address of the hello: a second socket would be a stranger. Non-blocking, so
//! the frame loop never waits on the network.

use std::io::{self, ErrorKind};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};

use bomber_domain::shared::PlayerId;
use bomber_protocol::{Action, ClientPacket, Hello, SeatBotRequest, ServerFrame};

pub struct Link {
    socket: UdpSocket,
    server: SocketAddr,
    seq: u8,
}

impl Link {
    /// Resolve `host` and open a socket towards it. IPv4 is preferred when a
    /// name resolves to both, since that is what LAN servers usually listen on.
    pub fn open(host: &str, port: u16) -> io::Result<Link> {
        let mut candidates: Vec<SocketAddr> = (host.trim(), port).to_socket_addrs()?.collect();
        candidates.sort_by_key(|a| !a.is_ipv4());
        let server = *candidates
            .first()
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "Adresse nicht gefunden"))?;
        let bind: SocketAddr = if server.is_ipv4() {
            "0.0.0.0:0".parse().unwrap()
        } else {
            "[::]:0".parse().unwrap()
        };
        let socket = UdpSocket::bind(bind)?;
        socket.set_nonblocking(true)?;
        Ok(Link {
            socket,
            server,
            seq: 0,
        })
    }

    pub fn server(&self) -> SocketAddr {
        self.server
    }

    pub fn send_hello(&self, name: &str) {
        let name = name.trim();
        let hello = if name.is_empty() {
            Hello::anonymous()
        } else {
            Hello::named(name)
        };
        self.send_raw(&hello.encode());
    }

    /// Every packet gets the next sequence number, or the server would drop
    /// repeats as duplicates.
    pub fn send_action(&mut self, player: PlayerId, action: Action) {
        let packet = ClientPacket::new(player, self.seq, action);
        self.seq = (self.seq + 1) & 0x0F;
        self.send_raw(&packet.encode());
    }

    /// Ask for a seat to be filled by a server bot, or for the bot to go.
    pub fn send_seat_bot(&mut self, player: PlayerId, seat: u8, enabled: bool) {
        let request = SeatBotRequest {
            player,
            seq: self.seq,
            seat: PlayerId::new(seat),
            enabled,
        };
        self.seq = (self.seq + 1) & 0x0F;
        self.send_raw(&request.encode());
    }

    fn send_raw(&self, bytes: &[u8]) {
        // A lost datagram is normal for UDP and nothing to retry here.
        let _ = self.socket.send_to(bytes, self.server);
    }

    /// Everything that has arrived, decoded. Undecodable datagrams are dropped.
    pub fn receive(&self) -> Vec<ServerFrame> {
        let mut frames = Vec::new();
        let mut buf = [0u8; 2048];
        // Bounded, so a flood cannot stall a frame.
        for _ in 0..512 {
            match self.socket.recv_from(&mut buf) {
                Ok((len, from)) => {
                    if from != self.server {
                        continue;
                    }
                    if let Ok(frame) = ServerFrame::decode(&buf[..len]) {
                        frames.push(frame);
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                // Windows reports an earlier ICMP "port unreachable" (server not
                // running yet) as a reset on the *next* receive. It says nothing
                // about the datagrams still queued, so keep reading.
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => break,
            }
        }
        frames
    }
}
