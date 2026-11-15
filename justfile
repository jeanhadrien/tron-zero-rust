# Project tasks. `just` lists them; `just <task> [args]` runs one.
# Install once: `winget install Casey.Just` (or `cargo install just`).
# Recipes run in PowerShell 7 on Windows and bash elsewhere.

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-Command"]
set shell := ["bash", "-euo", "pipefail", "-c"]

# Each Windows recipe line is its own pwsh -Command. Env a child process must
# see belongs in an [env] attribute, not an assignment earlier in the recipe:
# a split line would otherwise launch without it.

prod_manager := "https://server-manager.tronzero.dev/"
# `play` defaults; set the env var beforehand to override (0 = no simulated ping).
sim_latency := env("TRON_SIM_LATENCY_MS", "20")
local_config := env("SERVER_CONFIG", "crates/server/config/local.toml")

# List the tasks
default:
    @just --list

# --- play ------------------------------------------------------------------

# Server + bots + a client that joins by itself with a simulated 20 ms RTT; profile: dev | release
[env("TRON_AUTOCONNECT", "127.0.0.1:5000")]
[env("TRON_SIM_LATENCY_MS", sim_latency)]
[env("SERVER_CONFIG", local_config)]
[group('play')]
[windows]
play profile="dev":
    @Start-Process pwsh '-NoExit', '-Command', "`$Host.UI.RawUI.WindowTitle = 'tron-zero-server'; cargo run {{ if lowercase(profile) == "release" { "--release" } else if lowercase(profile) == "dev" { "" } else { error("profile is dev or release") } }} -p tron-zero-server"; \
    Start-Process pwsh '-NoExit', '-Command', "`$Host.UI.RawUI.WindowTitle = 'tron-zero-client'; cargo run {{ if lowercase(profile) == "release" { "--release" } else if lowercase(profile) == "dev" { "--features dev" } else { error("profile is dev or release") } }} -p tron-zero-client"; \
    Write-Host 'Server and client launched (simulated RTT {{ sim_latency }} ms). Close both windows to stop.'

# Server + bots + a client that joins by itself with a simulated 20 ms RTT; profile: dev | release
[env("TRON_AUTOCONNECT", "127.0.0.1:5000")]
[env("TRON_SIM_LATENCY_MS", sim_latency)]
[env("SERVER_CONFIG", local_config)]
[group('play')]
[unix]
play profile="dev":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo run {{ if lowercase(profile) == "release" { "--release" } else if lowercase(profile) == "dev" { "" } else { error("profile is dev or release") } }} -p tron-zero-server &
    server_pid=$!
    trap 'kill "$server_pid" 2>/dev/null || true' EXIT INT TERM
    echo "Server and client launching (simulated RTT {{ sim_latency }} ms). Close the client to stop."
    # The client keeps retrying until the server is up, so no sleep is needed.
    cargo run {{ if lowercase(profile) == "release" { "--release" } else if lowercase(profile) == "dev" { "--features dev" } else { error("profile is dev or release") } }} -p tron-zero-client

# Local server on the PUBLIC prod lobby (advertises 127.0.0.1) + a client on it
[env("ADVERTISED_HOST", "127.0.0.1")]
[env("ALLOW_LOOPBACK_ANNOUNCE", "1")]
[env("MANAGER_URL", "https://server-manager.tronzero.dev/")]
[env("TRON_BOT_DEBUG", "1")]
[group('play')]
[windows]
dev-cloud $SERVER_CONFIG="crates/server/config/dev-cloud.toml":
    @Write-Host 'Starting server from {{ SERVER_CONFIG }} against {{ prod_manager }}'; \
    Start-Process pwsh '-NoExit', '-Command', "`$Host.UI.RawUI.WindowTitle = 'tron-zero-server (cloud)'; cargo run -p tron-zero-server"; \
    Write-Host 'Waiting for registration...'; Start-Sleep 8; \
    Write-Host 'Current rooms:'; curl.exe -s '{{ prod_manager }}api/rooms'; Write-Host ''; \
    Write-Host 'Starting client against the same manager. Server browser, Refresh, Connect.'; \
    Write-Host 'NOTE: this advertises 127.0.0.1:5000 on a PUBLIC list. Ctrl-C the server window when done.'; \
    Start-Process pwsh '-NoExit', '-Command', "`$Host.UI.RawUI.WindowTitle = 'tron-zero-client (cloud)'; cargo run -p tron-zero-client"

# Local server on the PUBLIC prod lobby (advertises 127.0.0.1) + a client on it
[env("ADVERTISED_HOST", "127.0.0.1")]
[env("ALLOW_LOOPBACK_ANNOUNCE", "1")]
[env("MANAGER_URL", "https://server-manager.tronzero.dev/")]
[env("TRON_BOT_DEBUG", "1")]
[group('play')]
[unix]
dev-cloud $SERVER_CONFIG="crates/server/config/dev-cloud.toml":
    #!/usr/bin/env bash
    set -euo pipefail
    echo "Starting server from {{ SERVER_CONFIG }} against {{ prod_manager }}"
    cargo run -p tron-zero-server &
    server_pid=$!
    trap 'kill "$server_pid" 2>/dev/null || true' EXIT INT TERM
    echo "Waiting for registration..."; sleep 8
    echo "Current rooms:"; curl -s '{{ prod_manager }}api/rooms'; echo
    echo "NOTE: this advertises 127.0.0.1:5000 on a PUBLIC list. Close the client to stop both."
    cargo run -p tron-zero-client

# Headless server (official config unless SERVER_CONFIG is set)
[group('play')]
server *args:
    cargo run -p tron-zero-server {{ args }}

# Desktop client (dev feature: Bevy as a DLL for faster rebuilds)
[group('play')]
client *args:
    cargo run -p tron-zero-client --features dev {{ args }}

# Lobby service on :3001
[group('play')]
[working-directory('server-manager')]
lobby:
    bun run dev

# --- check -----------------------------------------------------------------

# What CI checks: fmt and clippy (`.github/workflows/lint.yml`)
[group('check')]
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --locked -- -D warnings

# Format the workspace
[group('check')]
fmt:
    cargo fmt --all

# Every test (nettest included, about 2 min)
[group('check')]
test *args:
    cargo nextest run --workspace {{ args }}

# Sim core tests (fast)
[group('check')]
test-sim *args:
    cargo nextest run -p tron-zero-shared {{ args }}

# Mechanics and golden traces
[group('check')]
test-lab *args:
    cargo nextest run -p tron-zero-lab {{ args }}

# Re-bless the lab golden traces after an intended change
[env("LAB_BLESS", "1")]
[group('check')]
bless *args:
    cargo nextest run -p tron-zero-lab {{ args }}

# Netcode over real UDP (about 2 min; never two at once)
[group('check')]
test-net *args:
    cargo nextest run -p tron-zero-nettest {{ args }}

# --- tools -----------------------------------------------------------------

# Headless lab runner, release build (e.g. `just lab tournament crates/lab/suites/rounds4.toml --quick`)
[group('tools')]
lab *args:
    cargo run --release -p tron-zero-lab -- {{ args }}

# samply CPU profile: server [-Secs 40] | client | lab [lab args] (Windows)
[group('tools')]
[windows]
profile target *args:
    & ./scripts/profile.ps1 {{ target }} {{ args }}

# Browser client into web/dist (args go to web/build.ps1, e.g. -CargoProfile release)
[group('tools')]
[windows]
web *args:
    & ./web/build.ps1 {{ args }}

# Browser client into web/dist (CARGO_PROFILE=release to iterate)
[group('tools')]
[unix]
web:
    bash web/build.sh
