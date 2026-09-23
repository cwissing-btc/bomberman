# Bomberman task runner.

default:
    @just --list

# Run the server: players on udp/47800, moderation web UI on http://127.0.0.1:8080.
server *ARGS:
    cargo run -p bomber-server -- {{ARGS}}

# Run the server, restarting on source changes.
watch:
    watchexec -r -e rs -- cargo run -p bomber-server

# Run the player client (Linux build needs libasound2-dev and X11 dev libs).
client *ARGS:
    cargo run -p bomber-client -- {{ARGS}}

# Server plus two bot clients that start matches on their own: a full demo.
arena:
    #!/usr/bin/env bash
    set -euo pipefail
    trap 'kill 0' EXIT
    cargo build -p bomber-server -p bomber-client
    ./target/debug/bomber-server &
    sleep 1
    ./target/debug/bomberman --host 127.0.0.1 --name Anna --bot --connect --autostart &
    sleep 1
    ./target/debug/bomberman --host 127.0.0.1 --name Ben --bot --connect &
    wait

# Run one example Python bot (standard library only).
pybot *ARGS:
    python3 clients/python/examples/wanderer.py {{ARGS}}

# Cross-build the Windows client into dist/Bomberman.exe.
windows:
    ./scripts/build-windows.sh

# Build and install the server as a systemd service (run on the Ubuntu host).
install-server:
    sudo ./deploy/ubuntu/install.sh

test:
    cargo test --workspace

lint:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    nix fmt
