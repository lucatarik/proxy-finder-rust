#!/usr/bin/env bash
# Avvia Proxy Finder: senza argomenti apre la GUI, con argomenti usa la riga di comando.
#   ./proxy-finder.sh                 GUI
#   ./proxy-finder.sh help            comandi CLI (vedi CLI.md)
#   ./proxy-finder.sh test -n 64      esempio CLI
# Ricompila in release se il binario manca o se i sorgenti sono più recenti.
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$DIR/target/release/proxy-finder"

needs_build() {
    [[ ! -x "$BIN" ]] && return 0
    [[ -n "$(find "$DIR/src" "$DIR/Cargo.toml" "$DIR/Cargo.lock" -newer "$BIN" -print -quit 2>/dev/null)" ]]
}

if needs_build; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "cargo non trovato: installa Rust da https://rustup.rs" >&2
        exit 1
    fi
    echo "compilo proxy-finder (release)…" >&2
    cargo build --release --quiet --manifest-path "$DIR/Cargo.toml"
fi

# senza display la GUI non può partire: mostra l'aiuto della CLI
if [[ $# -eq 0 && -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "nessun display: uso la riga di comando" >&2
    exec "$BIN" help
fi

exec "$BIN" "$@"
