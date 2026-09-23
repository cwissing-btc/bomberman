#!/usr/bin/env bash
# Build and install the Bomberman arena server as a systemd service on Ubuntu.
#
#   sudo ./deploy/ubuntu/install.sh            # build, install, start
#   sudo ./deploy/ubuntu/install.sh --no-ufw   # do not touch the firewall
#
# Everything the service uses lives under $PREFIX (default /opt/bomberman-l4d):
#   bin/bomber-server, server.toml, and the working directory itself.
#
# Run from a checkout of this repository. Safe to run again after a `git pull`:
# the binary and the unit are replaced, an existing config is kept.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PREFIX="${PREFIX:-/opt/bomberman-l4d}"
BIN="$PREFIX/bin/bomber-server"
CONF_DIR="$PREFIX"
UNIT=/etc/systemd/system/bomberman.service
SERVICE_USER=bomberman
USE_UFW=1
[[ "${1:-}" == "--no-ufw" ]] && USE_UFW=0

if [[ $EUID -ne 0 ]]; then
    echo "Bitte mit sudo ausführen." >&2
    exit 1
fi

# Build as the invoking user, not as root, so ~/.cargo and target/ stay theirs.
BUILD_USER="${SUDO_USER:-root}"
as_builder() { sudo -u "$BUILD_USER" -H bash -lc "$*"; }

if ! as_builder "command -v cargo" >/dev/null; then
    cat >&2 <<'MSG'
cargo wurde nicht gefunden. Rust einmalig für deinen Benutzer installieren:

    sudo apt install -y build-essential curl
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"

Dann dieses Skript erneut starten.
MSG
    exit 1
fi

echo "==> Server bauen (release)"
as_builder "cd '$REPO' && cargo build --release -p bomber-server"

echo "==> Dienstbenutzer '$SERVICE_USER'"
if ! id "$SERVICE_USER" >/dev/null 2>&1; then
    useradd --system --no-create-home --home-dir "$PREFIX" \
        --shell /usr/sbin/nologin "$SERVICE_USER"
fi

echo "==> Dateien installieren"
install -Dm755 "$REPO/target/release/bomber-server" "$BIN"
install -d -m755 "$CONF_DIR"
if [[ -f "$CONF_DIR/server.toml" ]]; then
    echo "    $CONF_DIR/server.toml existiert, bleibt unverändert"
    install -m644 "$REPO/config/server.toml" "$CONF_DIR/server.toml.default"
else
    install -m644 "$REPO/config/server.toml" "$CONF_DIR/server.toml"
fi
# The unit is written with the chosen prefix filled in.
sed "s|/opt/bomberman-l4d|$PREFIX|g" "$REPO/deploy/ubuntu/bomberman.service" > "$UNIT"
chmod 644 "$UNIT"

PORT="$(sed -n 's/^bind *= *"[^"]*:\([0-9]*\)".*/\1/p' "$CONF_DIR/server.toml" | head -1)"
PORT="${PORT:-47800}"

if [[ $USE_UFW -eq 1 ]] && command -v ufw >/dev/null && ufw status | grep -q "Status: active"; then
    echo "==> Firewall: UDP-Port $PORT freigeben"
    ufw allow "$PORT/udp" comment "Bomberman" >/dev/null
fi

echo "==> Dienst starten"
systemctl daemon-reload
systemctl enable bomberman.service >/dev/null
systemctl restart bomberman.service
sleep 1
systemctl --no-pager --lines=5 status bomberman.service || true

ADDR="$(hostname -I 2>/dev/null | awk '{print $1}')"
cat <<MSG

Fertig. Spieler verbinden sich mit:   ${ADDR:-<Server-IP>}  (UDP-Port $PORT)

  Konfiguration:  $CONF_DIR/server.toml   (danach: sudo systemctl restart bomberman)
  Protokoll:      journalctl -u bomberman -f
  Moderation:     ssh -L 8080:127.0.0.1:8080 <server>  und dann http://127.0.0.1:8080
MSG
