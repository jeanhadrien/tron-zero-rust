# Tron Zero (Rust) — Project Overview

Status: updated 2026-11-14.

Rust rewrite of **Tron Zero**, a multiplayer Lightcycle (Tron/Armagetron-style)
arena game. Players ride continuously-moving lightcycles on a 2D arena, leave
collidable trails behind them, boost by grinding close to *other* riders'
trails, and survive via a "rubber" buffer that drains when stuck facing an
obstacle and regenerates in open space. Death clears the trail; humans respawn
manually (Space). Enter opens the player chat.

## Tech stack

- **Engine/ECS:** Bevy 0.19 (`bevy_ecs`; full `bevy` on the client, `MinimalPlugins` on the server). Math via `glam::Vec2` (`f32` only in sim).
- **Networking:** Lightyear 0.28 — server-authoritative, tick-based, client prediction + rollback, state replication. One server entity carries two transports: **raw UDP** for the desktop client (default `0.0.0.0:5000`, env `PORT`; unauthenticated, unencrypted) and **WebTransport** for browsers (UDP `WEB_PORT`, default 5001; self-signed ECDSA cert pinned by the `certHash` the server announces). Lightyear netcode connect tokens for the UDP path are planned, not wired.
- **Client UI:** `bevy_egui` (menu, settings, server browser via `ehttp`, HUD, death overlay). Rendering is Bevy 2D.
- **Lobby:** the server announces itself over HTTP to `server-manager/` (Bun + Express, in this repo): register, heartbeat, unregister, with a per-room bearer token.
- **Other:** `serde` + `postcard` (wire), `tracing` (never `println!` in `crates/` non-test code; a clippy lint enforces it), `rand` (bot spawns, the client's network simulator and its random first-launch rider look only, never in the sim).

## Workspace layout

Each area has its own `AGENTS.md` (purpose, boundaries, map, tests). **Read the
one for the area you are about to work in, and keep it current in the same change.**

- **`crates/shared/`** — rules of the game (sim at 120 Hz, spawn, match flow) and wire protocol. Also holds the gameplay-invariant and wire-model summaries.
- **`crates/bots/`** — the bot brain: one planner, difficulty = budget knobs, personality = weight vector, rosters.
- **`crates/server/`** — headless authoritative server: admission, turns, rounds mode, bot seats, lobby announce.
- **`crates/client/`** — desktop (UDP) and browser (wasm32, WebTransport) client: connection, input, render, UI.
- **`crates/lab/`** — headless deterministic runner: scenarios, goldens, bot tournaments, tune, ladder. How an agent "plays" the game.
- **`crates/nettest/`** — real server and clients over loopback UDP; the end-to-end netcode tests.
- **`server-manager/`** — lobby service (Bun/TypeScript): rooms, accounts, tickets, Elo.

Lib + bin pairs keep logic in the lib so `nettest` can run the real apps.

## Architecture rules (cross-cutting; area rules are in the area files)

Code style is enforced by `cargo fmt -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` (workspace lints in `Cargo.toml`, test exemptions in `clippy.toml`, CI on PRs to `main` in `.github/workflows/lint.yml`); run both before committing.

- **Dependency direction:** `shared` <- `bots` <- (`server`, `lab`); `client` knows `shared` only. `shared/` knows nobody (no mention of a runner, even in comments). `server/` and `lab/` are two runners of the same game and never know each other. The test for each piece: is it a rule of the game that holds whoever runs it (rider reset, spawn search, match flow)? Then it lives in `shared/` (or `bots/` for bot logic) and the runners call it. Does it describe how one runner drives or observes the game (env vars, replication, sessions, ranked reports; traces, metrics, seeds, TOML files)? Then it stays in that runner. Never mirror one runner's logic in the other: move it down.
- **Determinism:** the sim and the bots use no `rand`, no wall clock, no unsorted `HashMap` iteration. Randomness stays server-side outside the predicted state, or is seeded.
- **Constants:** gameplay and tuning numbers are named consts, never inlined at call sites.
- **Logging:** `tracing` only (never `println!` in `crates/` library code). `warn!` = denied/degraded, `info!` = lifecycle, `debug!` = per-tick noise.
- **Gameplay rules:** `docs/specs/core-gameplay-mechanics.md` is normative. A game-rule change is JH's decision, never a side effect of bot or tooling work.

## Dev loops (details in `docs/guides/dev.md`)

- `cargo nextest run -p tron-zero-shared` — sim core (unit, property, rollback); fast.
- `cargo nextest run -p tron-zero-lab` — mechanics + golden traces; bless intended changes with `LAB_BLESS=1`.
- `cargo nextest run -p tron-zero-nettest` — netcode over real UDP (about 1-2 min; serialized by `.config/nextest.toml`).
- `scripts/play-local.bat [release]` (`scripts/play-local.sh` on Linux/macOS) — server + bots + auto-joining client with simulated 20 ms RTT (`TRON_SIM_*`, `TRON_AUTOCONNECT`).
- `bacon` (`bacon.toml`) — clippy watch loop by default; `bacon server` / `bacon client` restart on save.
- `scripts/dev-cloud.bat [name]` — local server announcing to the Cloud Run manager (`ALLOW_LOOPBACK_ANNOUNCE=1`, so it advertises 127.0.0.1 on a public list).
- `cd server-manager && bun run dev` — local lobby on `:3001`.

## Doc map

- `docs/specs/core-gameplay-mechanics.md` — gameplay rules, respect at all times.
- `docs/specs/rounds-mode.md` — rounds game mode (last rider standing over X rounds), `GAME_MODE=rounds`. Implemented in `crates/server/src/rounds.rs`; `play-local` runs it.
- `docs/specs/matchmaking-elo.md` — continuous Elo, leaderboard and placement. Implemented in `server-manager/src/rating/`, `results.ts`, `placement.ts`; the server reports eliminations from `crates/server/src/ranked.rs`. Tuning constants live in `rating/config.ts`.
- `docs/specs/netcode-design.md` — implemented netcode contract and measurements.
- `NETWORK.md` — open netcode/ops backlog.
- `docs/guides/run.md` — which commands to start for each way of running the game (local, web, prod lobby, GCP, release); start here.
- `docs/guides/dev.md` — dev loop, env vars, lab, smoke checks. `docs/guides/building.md` — prod builds, ports and env. `docs/guides/release.md` — releases and hosting for friends.
- `server-manager/README.md` — lobby API and Cloud Run deployment.
- `docs/specs/audio.md` — generative audio proposal (not implemented; open questions).
- `docs/specs/web.md` — browser client and WebTransport decisions; `web/README.md` — build, serve, deploy.
- `deploy/gce/` — game server VM (Container-Optimized OS) on GCP; image via `cloudbuild.server.yaml`.
- `crates/bots/rosters/default.toml` — the bots the server seats (five difficulty rungs plus the artist Escher), embedded in the server; rungs generated by `lab ladder crates/lab/tuning/duel.ladder.toml`, names edited by hand; `BOT_ROSTER=path` seats another file.
- `NOTES.md` — open feature ideas (audio, spectator mode). `EXPLORATION.md` (Armagetron ideas), `BOT-AI-EXPLORATION.md` (bot research) — not committed work; `BOT-AI-SPECS.md` records the settled bot decisions.

# Agent (you)

## Original JavaScript Codebase

The original repo is cloned alongside this Rust rewrite at `..\tron-zero-js`
(`C:\dev\tron-zero-js` when this repository is at `C:\dev\tron-zero-rust`).
Consult it for existing gameplay behavior and porting details.
Keep changes to the Rust repository unless explicitly asked to edit the JS code.

## Base Game Armagetron

The base game is available at https://github.com/ArmagetronAd/armagetronad

## Project Development

You may build, run and test the project. Reach for the cheapest tool that
answers the question:

- cargo check — fast compilation check without producing binaries (syntax + type checking)
- cargo clippy — linting (catches errors + idiomatic issues)
- cargo fmt — formatting
- cargo nextest run — `-p tron-zero-shared` for the sim core (fast), `--workspace` for everything; `cargo test --doc` for doctests
- cargo run — headless binaries and tools only (server, harnesses). Run
  long-lived processes in the background and stop them when done. Don't launch
  the windowed client unless asked: you can't play it at game speed.

The user may have `bacon` running; a "Blocking waiting for file lock" message
means you are sharing `target/` with it.

### Test loop

Builds, not edits, dominate iteration time here. Keep the loop tight:

- Run the narrowest test that covers the change first
  (`cargo nextest run -p tron-zero-shared polyline`), and the broad suites once,
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
- Measure bots with `lab tournament <suite> --quick` on at most half the
  cores; full suites are for a rare final check. Chase big differences.
- A new git worktree has its own `target/`: the first build is a full
  rebuild. Parallel agents: only one may run `tron-zero-nettest` at a time.

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