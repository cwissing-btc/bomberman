#!/usr/bin/env bash
# Cross-build the Windows client from Linux into dist/Bomberman.exe.
#
# Needs the MinGW-w64 cross compiler once:
#   Ubuntu/Debian:  sudo apt install gcc-mingw-w64-x86-64
# The Rust target is added automatically.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

TARGET=x86_64-pc-windows-gnu
if ! command -v x86_64-w64-mingw32-gcc >/dev/null; then
    echo "x86_64-w64-mingw32-gcc fehlt: sudo apt install gcc-mingw-w64-x86-64" >&2
    exit 1
fi
rustup target add "$TARGET" >/dev/null

cargo build --release -p bomber-client --target "$TARGET"
mkdir -p dist
cp "target/$TARGET/release/bomberman.exe" dist/Bomberman.exe
ls -l dist/Bomberman.exe
