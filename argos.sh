#!/bin/bash
# Launcher do Argos (Rust): roda o binário de release (recompila se o código mudou)
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/target/release/argos"
cargo build --release -q --manifest-path "$DIR/Cargo.toml" >&2 || exit 1
exec "$BIN" "$@"
