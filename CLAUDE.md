# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

A networked Bomberman built from three BTC hackathon projects: a Rust arena server (runs on Ubuntu as a systemd service) and a Rust/macroquad player client (ships as a single Windows `.exe`) that includes the lobby, start controls, the game view and a built-in bot. User-facing docs are in German (`README.md`); code comments and protocol docs are in English.

## Commands

```bash
cargo test --workspace                      # everything (just test)
cargo test -p bomber-application --test player_control   # one test file
cargo test -p bomber-client bot::tests::flees_from_flame # one test by path
cargo clippy --workspace --all-targets -- -D warnings    # lint, warnings are errors (just lint)
cargo run -p bomber-server -- --config config/server.toml
cargo run -p bomber-client -- --host 127.0.0.1 --name Anna --bot --connect
just arena                                  # server + two bot clients that autostart matches
./scripts/build-windows.sh                  # cross-build dist/Bomberman.exe (needs gcc-mingw-w64-x86-64)
sudo ./deploy/ubuntu/install.sh             # on the Ubuntu host: build, install, systemd, ufw
```

Linux builds of `bomber-client` link against `libasound` (package `libasound2-dev`). Without it, only the client fails to link; server crates are unaffected. X11/GL are loaded at runtime.

Do not run `cargo fmt --all`: the original server crates are not rustfmt-clean and a workspace-wide format creates a large unrelated diff. Format only files you touch (`rustfmt --edition 2021 <file>`).

## Architecture

Hexagonal, dependencies point inward:

- `bomber-domain` — the rules only. Pure and deterministic (same seed + inputs = same match). No I/O, no wire format. Sudden-death `closing_order` lives here and is reused by the client.
- `bomber-protocol` — byte layouts for the UDP protocol; the single source of truth for `BOT_GUIDE.md`. Depends only on the domain, so the client reuses it directly.
- `bomber-application` — `ArenaSession`: admission, the tick, delivery policy (keyframe vs. delta), moderation, player lobby requests. Synchronous and socket-free: `tick()` returns a `TickReport` of frames to send, which is why whole matches are unit-tested without networking.
- `bomber-server` — composition root: UDP endpoint (`udp/`), 60 Hz tick loop (`runtime/`), axum web/WebSocket for spectators and moderation (`web/`). The `Registry` binding socket address ↔ seat is the only authentication.
- `bomber-bot` — pure bot logic shared by client and server: `world.rs` (state rebuilt from keyframes/deltas, emits `FxEvent`s), `bot.rs` (port of the hackathon Python bot) and `Driver` (frames in, action out). Its tests run without `libasound`.
- `bomber-client` — macroquad app. `session.rs` (connection state machine, loss handling, send cadence) is pure logic with tests; `world`/`bot` are re-exported from `bomber-bot`; `app.rs` wires input, network and drawing; `board.rs`/`screens.rs`/`fx.rs`/`ui.rs` are presentation. All assets are `include_bytes!`-embedded; sounds are synthesized in `sound.rs`.

## Protocol constraints that shape everything

- The uplink is **two bytes** `[player_id][(seq<<4)|action]`. No room for acks or tokens: the server sends a keyframe every 30 ticks and deltas naming their `base_tick`; a client applies a delta only if it holds exactly that tick, otherwise waits for the next keyframe.
- The server keeps only the **newest packet per tick window**. A client must never send several packets in one burst (a later move would overwrite a bomb). `app.rs` sends at most one packet per frame and 60/s.
- Uplink codes 10–13 are lobby requests (Start/Pause/Resume/Abort) handled immediately by `ArenaSession::player_request`, deduplicated by the sequence nibble, disabled by `lobby.player_control = false`. Start on the results screen means reset + start.
- `LOBBY_STATUS` has a backward-compatible tail after byte 11 (flags, min players, per-seat occupied/stale/name). It is broadcast whenever the lobby changes and every 30 ticks while paused (a paused match otherwise sends nothing and clients would time out after 3 s).
- An aborted match emits `MATCH_END` (reason aborted) on the next tick via `pending_end`.
- Uplink code 14 (`SeatBot`) is the one 4-byte action packet: `[id][(seq<<4)|14][seat][0|1]`. It sets a seat's `bot_fill`; `ArenaSession` then seats/removes server bots in `sync_bots`. Server bots come through the `ports::BotFactory` port (the real one is `bomber-server/src/bots.rs`, wrapping `bomber_bot::Driver`), observe the same frames a client in the seat would get (`show_bots`) and submit moves at the start of each match tick (`bot_moves`). Bots sit only between matches and only while at least one human is seated; when the last human leaves they all leave, fills revert to `config.bot_seats`, and a running match is aborted.
- Humans silent for `drop_after_ticks` (30 s) are released; `TickReport::released` tells the tick loop to drop their registry binding. A repeated hello counts as a sign of life (`heard_from`).
- New players are admitted in `Open` and `MatchOver` (not `Locked`, `Countdown`, `Running`), because a player's Start on the results screen skips the open lobby.

Any change to a byte layout must update `BOT_GUIDE.md` and the round-trip tests in `crates/bomber-protocol/tests/`.

## Client rendering notes

- The board is drawn at native 64 px/cell into a render target, then scaled into the window. macroquad's `Camera2D::from_display_rect` flips y for the screen; for a render target the zoom's y sign must be flipped back (see `app.rs`), or the board is upside down.
- A moving player's `x/y` is already the destination cell; the drawn position trails back along `dir` by `1 - progress/ticks_per_cell`.
- Headless visual check: run under `Xvfb :77` and screenshot with `import -window root` (ImageMagick). `--autostart` and `--debug` (F3 overlay) help in unattended runs.
