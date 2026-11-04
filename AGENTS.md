# Tron Zero (Rust) — Project Overview

Status: updated 2026-11-02.

Rust rewrite of **Tron Zero**, a multiplayer Lightcycle (Tron/Armagetron-style)
arena game. Players ride continuously-moving lightcycles on a 2D arena, leave
collidable trails behind them, boost by grinding close to *other* riders'
trails, and survive via a "rubber" buffer that drains when stuck facing an
obstacle and regenerates in open space. Death clears the trail; humans respawn
manually (Space/Enter).

## Tech stack

- **Engine/ECS:** Bevy 0.19 (`bevy_ecs`; full `bevy` on the client, `MinimalPlugins` on the server). Math via `glam::Vec2` (`f32` only in sim).
- **Networking:** Lightyear 0.28 — server-authoritative, tick-based, client prediction + rollback, state replication. One server entity carries two transports: **raw UDP** for the desktop client (default `0.0.0.0:5000`, env `PORT`; unauthenticated, unencrypted) and **WebTransport** for browsers (UDP `WEB_PORT`, default 5001; self-signed ECDSA cert pinned by the `certHash` the server announces). Lightyear netcode connect tokens for the UDP path are planned, not wired.
- **Client UI:** `bevy_egui` (menu, settings, server browser via `ehttp`, HUD, death overlay). Rendering is Bevy 2D.
- **Lobby:** the server announces itself over HTTP to `server-manager/` (Bun + Express, in this repo): register, heartbeat, unregister, with a per-room bearer token.
- **Other:** `serde` + `postcard` (wire), `tracing` (never `println!` in `crates/` non-test code), `rand` (bot spawns and the client's network simulator only, never in the sim).

## Workspace layout

`crates/` (all lib + bin pairs keep logic in the lib so `nettest` can run the real apps):

- **`shared/` (lib)** — the simulation core and wire protocol. Pure Bevy ECS: no rendering, no wall clock. Both sides run `simulate_players` at **120 Hz** (`TICK_HZ`). `physics.rs` (`step_rider`, pure), `constants.rs` (gameplay numbers), `systems/` (turn → sense → move → rubber; order is load-bearing), `trail/`, `spawn.rs`, `protocol.rs` (components, messages, channels, `PROTOCOL_ID`, turn constants), `components/`.
- **`bots/` (lib)** — bot brain over a `WorldSnapshot` (sensing, memory, forecast, territory, strategies, Elo knobs). Deterministic: no `rand`, no wall clock. Shared by the server and the lab.
- **`server/` (lib + bin)** — headless authoritative server. `add_core` (lib.rs) wires Lightyear and the chain `handle_respawns → apply_turn_commands → simulate_players`; `main.rs` adds bots, logging and the announcer. Modules: `admission` (hello handshake, capacity), `session` (heartbeat/leave/lease), `turns` (sequenced turn commands, acks), `budget` (per-link abuse limits), `systems` (connect/disconnect, arena, respawn queue), `announce` (manager HTTP), `bot/` (ECS glue).
- **`client/` (lib + bin)** — desktop client (UDP) and browser client (wasm32, WebTransport; `platform.rs` holds the target splits, build with `web/build.ps1` or `web/build.sh`, design in `docs/web.md`). `connection` (lifecycle, readiness, autoconnect, quit), `input/` (keyboard queue, `TurnOutbox` redundant delivery), `menu`/`settings`/`browser`/`config` (UI, key bindings, room list, persisted preferences), `sim` (latency simulator), `render/` (arena, trails, riders, camera, HUD, death, bot debug).
- **`lab/` (lib + bin)** — headless deterministic scenario runner over the real `simulate_players`. TOML scenarios in `crates/lab/scenarios/` (goldens `*.golden.jsonl`); JSONL trace + PNG output. **This is how an agent "plays" the game**: edit a scenario, re-run from tick 0, read trace and PNG. See `docs/dev.md` § Lab.
- **`nettest/` (lib + tests)** — real server and client(s) in one process over loopback UDP with a link conditioner; scripted key presses, trace assertions (`tests/turns.rs`, `session.rs`) and arrival measurements (`measure.rs`, `--ignored`). Uses wall-clock time, so a failure should be re-run alone before it is believed.
- **`server-manager/` (Bun/TypeScript)** — in-memory lobby service. API, env vars and deployment in its README; `bun run typecheck`.

## Netcode model (short version)

Forward-only authoritative server, immediate local prediction, client reconciliation only — no server rewind. Own rider is `Predicted`; opponents and bots are step-interpolated. A trail is one replicated+predicted `Trail(Vec<Vec2>)` component, **not** child entities (this deliberately diverges from `PLAN.md`).

- **Turns** are sequenced commands (`TurnCommand { seq, direction, generation, tick }`), not Lightyear inputs: every undecided command is resent in each tick's unreliable `TurnBatch` (<= 16 commands, repeated for 48 ticks), the server decides each seq once, executes it at its intended tick, within a 12-tick late window, or rejects it with a reason, and acks cumulatively. Lightyear's native `PlayerInput` only feeds client prediction; the server overwrites it for humans.
- **Session:** the first message is a `SessionHello { protocol, name }`; the server admits (sanitized unique name) or rejects (`VersionMismatch`, `ServerFull`). Nothing replicates to a link before admission. Heartbeats, leave and respawn travel on ordered-reliable channels (`SessionChannel`, `RespawnChannel`); respawns are server-applied and tagged by `LifeGeneration`.
- **Hardening:** per-link `LinkBudget`, capped unadmitted links, `PROTOCOL_ID` = hand-bumped `PROTOCOL_VERSION` + fingerprint of every sim constant (a physics retune changes it by itself; a *logic* change with the same constants needs a manual bump).

Implemented contract and measurements: `docs/netcode-design.md`. Open items: `NETWORK.md`.

## Gameplay invariants (`docs/core-gameplay-mechanics.md` is normative)

120 ticks/s, base speed 360 u/s, 2000x2000 arena, one turn per tick, trail cap `TRAIL_MAX_LENGTH` (2400), rubber max 120 with cubic-in-cruise-speed drain and slow regen, Zeno-style slowdown within 12 units of a front obstacle, capped inertial speed (`MAX_SPEED_MULT` 2.0) fed by grinding within `SLIDE_DISTANCE` of others' trails (own trail never grinds; arena walls pay 0.4x), deterministic spawn/safe-respawn search, manual human respawn only. Sim arithmetic uses `+ - * /` only (no `powi`/`exp`) so client and server stay bit-identical.

## Dev loops (details in `docs/dev.md`)

- `cargo test -p tron-zero-shared` — sim core (unit, property, rollback); fast.
- `cargo test -p tron-zero-lab` — mechanics + golden traces; bless intended changes with `LAB_BLESS=1`.
- `cargo test -p tron-zero-nettest` — netcode over real UDP (about 1-2 min; serialized).
- `play-local.bat [release]` — server + bots + auto-joining client with simulated 20 ms RTT (`TRON_SIM_*`, `TRON_AUTOCONNECT`).
- `dev-watch.bat` — `cargo watch` for server and client (sets `TRON_BOT_DEBUG=1`).
- `dev-cloud.bat [name]` — local server announcing to the Cloud Run manager (`ALLOW_LOOPBACK_ANNOUNCE=1`, so it advertises 127.0.0.1 on a public list).
- `cd server-manager && bun run dev` — local lobby on `:3001`.

## Doc map

- `docs/core-gameplay-mechanics.md` — gameplay rules, respect at all times.
- `docs/netcode-design.md` — implemented netcode contract and measurements.
- `NETWORK.md` — open netcode/ops backlog.
- `docs/run.md` — which commands to start for each way of running the game (local, web, prod lobby, GCP, release); start here.
- `docs/dev.md` — dev loop, env vars, lab, smoke checks. `docs/building.md` — prod builds, ports and env. `docs/release.md` — releases and hosting for friends.
- `server-manager/README.md` — lobby API and Cloud Run deployment.
- `docs/web.md` — browser client and WebTransport decisions; `web/README.md` — build, serve, deploy.
- `deploy/gce/` — game server VM (Container-Optimized OS) on GCP; image via `cloudbuild.server.yaml`.
- `PLAN.md` — original rewrite plan; **current code wins** (see its top note). `EXPLORATION.md` (Armagetron ideas), `BOT-AI-*.md` (bot research) — not committed work. `STYLE.md` — enforced style (fmt + clippy `-D warnings`).

# Agent (you)

## Original JavaScript Codebase

The original repo is cloned alongside this Rust rewrite at `..\tron-zero-js`
(`C:\dev\tron-zero-js` when this repository is at `C:\dev\tron-zero-rust`).
Consult it for existing gameplay behavior and porting details; `PLAN.md` is
the rewrite plan, not a substitute for checking the original implementation.
Keep changes to the Rust repository unless explicitly asked to edit the JS code.

## Base Game Armagetron

The base game is available at https://github.com/ArmagetronAd/armagetronad

## Project Development

You may build, run and test the project. Reach for the cheapest tool that
answers the question:

- cargo check — fast compilation check without producing binaries (syntax + type checking)
- cargo clippy — linting (catches errors + idiomatic issues)
- cargo fmt — formatting
- cargo test — `-p tron-zero-shared` for the sim core (fast), `--workspace` for everything
- cargo run — headless binaries and tools only (server, harnesses). Run
  long-lived processes in the background and stop them when done. Don't launch
  the windowed client unless asked: you can't play it at game speed.

The user may have `cargo watch` running (`dev-watch.bat`); a "Blocking waiting
for file lock" message means you are sharing `target/` with it.

### Test loop

Builds, not edits, dominate iteration time here. Keep the loop tight:

- Run the narrowest test that covers the change first
  (`cargo test -p tron-zero-shared polyline`), and the broad suites once,
  at the end.
- Don't pipe long cargo commands into `grep`/`tail`: it hides progress and
  lock waits, and a command moved to the background shows an empty output
  until it ends. Run long builds in the background with their full output
  and read the summary when they finish.
- `tron-zero-nettest` takes about two minutes and binds real ports: run it
  once, alone, at the end, and never start a second copy while one runs
  (overlapping runs fail spuriously).
- Line endings are LF everywhere except `*.bat` (`.gitattributes`); edit
  with the Edit tool rather than ad-hoc rewrite scripts.

## Generic Rust Development

Below are generic Rust gotchas that can help you. Don't get too caught up in those.

### Gotchas

- Moves are real. A non-Copy value passed anywhere leaves the old binding dead.
- No &mut coexisting with any other reference to the same data.
- String vs &str. String is owned and heap-allocated; &str is borrowed. You cannot return a &str pointing at something you just created.
- ? propagates errors. The type is Result<T, E>, not exceptions.
- No null, no inheritance. Use Option<T>, enums for sum types, traits for polymorphism, composition over inheritance.
- Shadowing is idiomatic. let x = x.trim(); is normal.
- Iterators are lazy. Nothing runs until they are consumed.
- Drop order. Locals drop in reverse declaration order. Struct fields drop in declaration order, not reverse.

### Performance

- Allocations usually dominate over instruction-level tweaks. Pre-allocate (with_capacity), reuse buffers, and avoid format!() in hot paths.
- Do not benchmark debug builds. --release is a different language.
- Vec beats linked lists. Cache locality wins.

### Maintainability

- Encode invariants in types so illegal states are unrepresentable. Prefer newtypes over raw primitives.
- pub sparingly. The default is private; use pub(crate) for internals.
- #[derive] deliberately. Debug almost always, Clone only when you mean it, Copy only for small plain data.
- Traits, not deep hierarchies. Keep them thin and composable (Read / Write).
- Rc<RefCell<_>> everywhere means the ownership model is wrong.
- Unsafe stays small. Isolate it and write the invariants in // SAFETY: comments.