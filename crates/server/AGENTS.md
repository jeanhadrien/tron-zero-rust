# `server/` — headless authoritative server

Root rules: `../../AGENTS.md`. Netcode contract: `docs/specs/netcode-design.md`. Matches: `docs/specs/match-rules.md`.

## Purpose

Run the real game: admit links, apply turns, step the sim, run the match,
seat bots, announce to the lobby. Lib + bin: logic stays in the lib so
`nettest` can run the real app.

## Boundaries

- Depends on `shared/`, `bots/` and `scene/`. Never on `lab/` or `client/`. The client runs `embedded` (a server on its own thread, local-play rules, no announce/ranked/web): keep it free of process-wide parts (`exit`, signals, env vars, logger).
- The only place with secrets, manager HTTP, admission and ranked reporting.
- Game rules do not live here. If the lab would need the same logic (rider
  reset, spawn inputs, round flow, bot driving), it belongs in `shared/` or
  `bots/` and the server calls it.
- `rand` is allowed here, outside the predicted state (bot brain seeds).

## Map

- `lib.rs` — `new_app` (paced headless app) and `add_game` (rules, player cap, `add_core`, bots) are the one way to build a server, used by `main.rs` and the hosted server; `add_core` wires Lightyear, the chain `handle_respawns → apply_turn_commands → simulate_players → count_scores → resolve_deaths`, and after it the `Director`: `game::advance_match` (a match) or `tutorial::advance_tutorial`. `main.rs` — logging, env, announcer, web, ranked, signals.
- `admission` (hello, capacity), `session` (heartbeat, leave, lease), `turns` (sequenced commands, acks), `budget` (per-link abuse limits).
- `systems` (connect/disconnect, arena, respawn queue), `game` (runs `shared::MatchFlow` on every server: scores, placement, bots off while no human is here, replicated `MatchStatus`; respawns ask `game::may_spawn`, which follows the `Respawns` resource `add_core` sets from the director: `ByMatch` or `Never`).
- `tutorial` — the tutorial director: `load` (a `scene::step` file, refused while a step has riders other than the player, until the server seats scripted riders and bots), attempts (retire everyone, set the step's walls on the replicated `WallSegments`, countdown, placement in a new life, `scene::step::Judge`, result). Walls change only while every rider is retired, followed by the countdown, so no tick (or client re-simulation) sees a wall change under a living rider, a constant replicated `MatchStatus`; status out through `embedded::HostHandle::tutorial`, never on the wire. Hosted only (`HostMode::Tutorial`, loopback, one player).
- `config` — the server config file (room, `[match]` and `[round]` rules, bots); `config/official.toml` is embedded, `config/local.toml` (`just play`) and `dev-cloud.toml` (`just dev-cloud`) are the local configs.
- `bot/` — seats the roster named by the config, drives brains through `bots::drive`, respawns a dead bot as the same entity on a safe spot, debug overlay.
- `embedded` (`spawn` -> `HostHandle`, the server a client hosts), `look`, `chat`, `probe`, `web` (WebTransport, certificate rotation), `announce` (manager HTTP), `ranked` and `elimination` (results reporting).

## Rules

- A new `FixedUpdate` system states its order against `simulate_players` with a comment saying why.
- Every message a client can send freely gets a per-link budget in `budget.rs`.
- Logging: `warn!` = denied/degraded, `info!` = lifecycle, `debug!` = per-tick noise. Install the log subscriber before reading config.
- Game settings (room, match rules, bots) come from the config file (`SERVER_CONFIG`, else the embedded official one), every key required unless documented optional; deployment settings and secrets (ports, advertised host, manager, `RANKED_*`) from env vars. Never put one in the other. Both are read at start; an invalid value stops the server (`main` exits with code 2; the lib only returns `Err`), it never silently runs other rules. Full list: `docs/guides/building.md`.

## Test

- `cargo nextest run -p tron-zero-server` for unit tests (state machines, respawn, budgets).
- `cargo nextest run -p tron-zero-nettest` for anything a client can observe: once, alone, at the end.
- Smoke: run the release server alone for 20 s and read its log (`SERVER_CONFIG=crates/server/config/local.toml` with a short `round.secs` in a copy exercises a full match).
