#!/usr/bin/env bash
# One-click local test: server (with its bot roster) + client, the client
# auto-joining with a simulated ~20 ms round-trip ping.
#
#   scripts/play-local.sh            dev profile (deps are already fully optimised)
#   scripts/play-local.sh release    --release build (slower first build, fastest game)
#
# Override before running, e.g.  TRON_SIM_LATENCY_MS=80 scripts/play-local.sh   (0 = no simulation)
# Also honoured: TRON_SIM_JITTER_MS, TRON_SIM_LOSS_PCT (percent, per direction).
#
# The server runs in the background of this shell, the client in the
# foreground; closing the client (or Ctrl+C) stops the server too.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

command -v cargo >/dev/null || { echo "cargo not on PATH. Open a new shell after rustup install." >&2; exit 1; }

# Dev client links Bevy as a dylib (crates/client "dev" feature) for faster rebuilds.
cargo_flags=()
client_flags=(--features dev)
if [[ "${1:-}" == "release" ]]; then
  cargo_flags+=(--release)
  client_flags=()
fi

export TRON_SIM_LATENCY_MS="${TRON_SIM_LATENCY_MS:-20}"
export TRON_AUTOCONNECT="127.0.0.1:5000"
# Game settings: 5 last-standing rounds. Point SERVER_CONFIG
# at another file to play other rules (docs/specs/match-rules.md).
export SERVER_CONFIG="${SERVER_CONFIG:-crates/server/config/local.toml}"

cargo run "${cargo_flags[@]}" -p tron-zero-server &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null || true' EXIT INT TERM

echo "Server and client launching (simulated RTT ${TRON_SIM_LATENCY_MS} ms). Close the client to stop."
# The client keeps retrying until the server is up, so no sleep is needed.
cargo run "${cargo_flags[@]}" "${client_flags[@]}" -p tron-zero-client
