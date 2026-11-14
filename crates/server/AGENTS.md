# `server/` — headless authoritative server

Root rules: `../../AGENTS.md`. Netcode contract: `docs/specs/netcode-design.md`. Rounds: `docs/specs/rounds-mode.md`.

## Purpose

Run the real game: admit links, apply turns, step the sim, run the match,
seat bots, announce to the lobby. Lib + bin: logic stays in the lib so
`nettest` can run the real app.

## Boundaries

- Depends on `shared/` and `bots/`. Never on `lab/` or `client/`.
- The only place with secrets, manager HTTP, admission and ranked reporting.
- Game rules do not live here. If the lab would need the same logic (rider
  reset, spawn inputs, round flow, bot driving), it belongs in `shared/` or
  `bots/` and the server calls it.
- `rand` is allowed here, outside the predicted state (bot brain seeds).

## Map

- `lib.rs` — `add_core` wires Lightyear and the chain `handle_respawns → apply_turn_commands → simulate_players → resolve_deaths → advance_match` (the last in rounds mode only). `main.rs` — bots, logging, announcer.
- `admission` (hello, capacity), `session` (heartbeat, leave, lease), `turns` (sequenced commands, acks), `budget` (per-link abuse limits).
- `systems` (connect/disconnect, arena, respawn queue), `rounds` (applies `shared::MatchFlow`: scores, placement, replicated `MatchStatus`).
- `config` — the server config file (room, game mode, bots); `config/official.toml` is embedded, `config/local.toml` and `dev-cloud.toml` serve the scripts.
- `bot/` — seats the roster named by the config, drives brains through `bots::drive`, respawns a dead bot as the same entity on a safe spot, debug overlay.
- `look`, `chat`, `probe`, `web` (WebTransport, certificate rotation), `announce` (manager HTTP), `ranked` and `elimination` (results reporting).

## Rules

- A new `FixedUpdate` system states its order against `simulate_players` with a comment saying why.
- Every message a client can send freely gets a per-link budget in `budget.rs`.
- Logging: `warn!` = denied/degraded, `info!` = lifecycle, `debug!` = per-tick noise. Install the log subscriber before reading config.
- Game settings (room, mode, bots) come from the config file (`SERVER_CONFIG`, else the embedded official one), every key required; deployment settings and secrets (ports, advertised host, manager, `RANKED_*`) from env vars. Never put one in the other. Both are read at start; an invalid value stops the server (exit code 2), it never silently runs another mode. Full list: `docs/guides/building.md`.

## Test

- `cargo nextest run -p tron-zero-server` for unit tests (state machines, respawn, budgets).
- `cargo nextest run -p tron-zero-nettest` for anything a client can observe: once, alone, at the end.
- Smoke: run the release server alone for 20 s and read its log (`SERVER_CONFIG=crates/server/config/local.toml` with a short `round_secs` in a copy exercises a full match).
