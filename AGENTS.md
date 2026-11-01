# Tron Zero (Rust) — Project Overview

Rust rewrite of **Tron Zero**, a multiplayer Lightcycle (Tron/Armagetron-style)
arena game. Players ride continuously-moving lightcycles on a 2D arena, leave
collidable trails behind them, boost by grinding close to walls, and survive
via a "rubber" buffer that drains when stuck facing an obstacle and regenerates
in open space. Death clears the trail; humans respawn manually (Space/Enter).

## Tech stack

- **Engine/ECS:** Bevy 0.19 (`bevy_ecs`, full `bevy` on client, `MinimalPlugins` on server). Math via `glam::Vec2` (`f32` only in sim).
- **Networking:** Lightyear 0.28 — server-authoritative, tick-based, with client prediction + rollback and state replication. Dev transport is raw UDP (`127.0.0.1:5000`); WebTransport (QUIC) for browser/prod is planned, not wired.
- **Client UI:** `bevy_egui` (menu, server browser via `ehttp`, HUD, death overlay). Rendering is Bevy 2D (arena grid, trails, rider sprites, follow camera).
- **Lobby:** the Rust server announces itself to the existing Node `server-manager` via HTTP (`POST /api/rooms`, heartbeat, `DELETE` on shutdown) — lobby stays in JS.
- **Other:** `serde` + `postcard` (wire), `tracing` (logging, never `println!` in `crates/`), `rand` (bots only, never in predicted sim).

## Workspace layout (`crates/`)

- **`shared/` (lib)** — the simulation core. Pure Bevy ECS, no networking/rendering/`rand`/wall-clock. Both client and server run the same `simulate_players` system at **120 Hz** (`TICK_HZ`). Contains: `components/` (player, trail, arena newtypes like `Rubber`, `SpeedMult`, `Position`), `systems/player.rs` (lifecycle + turn → sense → move → rubber; order is load-bearing), `systems/arena.rs`, `trail/` (tail-trim, arc length), `spawn.rs` + `math.rs` (safe-spawn search, rotations), `protocol.rs` (lightyear registration: what is `predict()` vs `replicate_once()` vs local-only), `constants.rs`.
- **`bots/` (lib)** — the bot brain, split out of the server so the lab can run it: `WorldSnapshot` (+ its ECS builder), ray/BFS sensing, memory, physics forecaster, territory analysis, strategies (survivalist, trapper, adaptive, territorialist, duelist) and the Elo knob curve. `Brain::think` is one decision epoch. Deterministic: no `rand`, no wall clock.
- **`server/` (bin)** — headless authoritative server. FixedUpdate chain: `handle_respawns → bot_brain_input → simulate_players → bot_spawner`. Plus `bot/` (ECS glue: roster, staggered scheduling, spawning, `BotDebug` writer), `session.rs` (heartbeat/leave/lease), `announce.rs` (manager HTTP), `systems.rs` (connect/disconnect observers, arena spawn, respawn queue).
- **`client/` (bin)** — native desktop client (WASM target not wired yet). Owns connection lifecycle (`connection.rs`: connect/cancel/timeout/disconnect, readiness gating), keyboard queue (`input/`), rendering + camera (`render/`), egui menu/HUD/death overlay (`menu.rs`, `render/hud.rs`, `render/death.rs`), server browser (`browser.rs`), keybindings (`settings.rs`).
- **`lab/` (lib + bin)** — headless, deterministic scenario runner over the real `simulate_players` (no networking). TOML scenarios in `crates/lab/scenarios/` script riders per tick or hand them to the bot brain (seeded bot arenas); output is a JSONL event trace (turns, rubber zone, slides, deaths with cause, respawns) and PNG snapshots. Tests assert mechanics and compare golden traces. **This is how an agent "plays" the game**: write or edit a scenario, re-run from tick 0, read the trace and PNG. See `docs/dev.md` § Lab.

## Netcode model (short version)

Forward-only authoritative server, immediate local prediction, client
reconciliation only — no server rewind. Own rider is `Predicted`; opponents/bots
are interpolated (step interpolation keeps cardinal corners coherent). Trail is
a single replicated+predicted `Trail(Vec<Vec2>)` component (oldest → head),
**not** child entities — this deliberately diverges from `PLAN.md`. Turns are
one-shot Lightyear inputs; respawn/session/leave are ordered-reliable messages
(`RespawnChannel`, `SessionChannel`), never inputs. Human respawn is
server-applied and tagged by `LifeGeneration` (connection nonce + life counter)
so stale turns can't leak across lives. See `docs/netcode-design.md` for the
implemented contract and `NETWORK.md` for the hardening backlog (one-shot
delivery under loss, collision fairness, diagnostics — the P0 turn-repeat risk
is known and unsolved).

## Gameplay invariants (`docs/core-gameplay-mechanics.md` is normative)

120 ticks/s, base speed 360 u/s, one turn per tick, trail cap `TRAIL_MAX_LENGTH`,
rubber max 120 with cubic-speed drain and slow regen, slide boost near walls,
Zeno-style slowdown within 12 units, deterministic spawn/safe-respawn search
(margin/clearance/corridor), manual human respawn only.

## Doc map

- `PLAN.md` — original rewrite plan. Aspirational in places (child-entity trails, bot stack, WebTransport, audio/chat); **current code wins** where they disagree.
- `docs/core-gameplay-mechanics.md` — gameplay rules, respect at all times.
- `docs/netcode-design.md` — what netcode is actually implemented vs goals.
- `NETWORK.md` — netcode hardening backlog + next-session TODO (paused 2026-10-01).
- `docs/dev.md` — Windows dev loop + gameplay smoke checks. `docs/building.md` — prod builds (Linux server, Windows client). `EXPLORATION.md` — Armagetron ideas, not committed work. `STYLE.md` — enforced style (fmt + clippy `-D warnings`).

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