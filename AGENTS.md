# Tron Zero (Rust) — Project Overview

Status: updated 2026-11-08.

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

`crates/` (all lib + bin pairs keep logic in the lib so `nettest` can run the real apps):

- **`shared/` (lib)** — the simulation core and wire protocol. Pure Bevy ECS: no rendering, no wall clock. Both sides run `simulate_players` at **120 Hz** (`TICK_HZ`). `physics.rs` (`step_rider`, pure), `constants.rs` (gameplay numbers), `systems/` (turn → sense → move → rubber; order is load-bearing), `components/` (incl. `trail.rs`, the rollbackable trail), `spawn.rs`, `math.rs` (rotations, arena walls, seeded spawn), `polyline.rs` (render-only arc-length helpers), `probe.rs` (UDP server-discovery datagram), `match_rules.rs` (rounds mode rules: respawn, end condition, rounds; plain data, outside the sim), `elimination.rs` (rated elimination events from the death log), `protocol.rs` (components, messages, channels, `PROTOCOL_ID`, turn constants).
- **`bots/` (lib)** — bot brain over a `WorldSnapshot`: candidate maneuvers (`planner`, `plan`) scored by a weighted objective (`objective`, personality = weights) over a timed flood-fill of room (`space`), the best ones verified with the real physics (`forecast`); a territory race on a coarse grid (`race`) and a model of each opponent (rides straight, the target also gets a reply check) feed the fight terms `pressure`, `control` and `engage`, gated by survival (`risk`) and worn down as a round ages; a speed model (`grind`: speed earned beside trails, walls and foes' trails, never the bot's own) with lane candidates aimed beside them, a rubber term (burn against what the line faces, scaled by what is left), and a round heat (only for bots told they play a round: bolder fight terms at once, risk below the calm window only for a kill chance, less margin late) so rounds end; `Personality::Passive` turns the fight and grind terms off (reference opponent); difficulty = budget knobs (`config`: reaction, horizon, variety, noise). Deterministic: no `rand`, no wall clock. Shared by the server and the lab.
- **`server/` (lib + bin)** — headless authoritative server. `add_core` (lib.rs) wires Lightyear and the chain `handle_respawns → apply_turn_commands → simulate_players → resolve_deaths → advance_match` (the last only in rounds mode: `rounds` holds the round state machine, rider placement and the replicated `MatchStatus`); `main.rs` adds bots, logging and the announcer. Modules: `admission` (hello handshake, capacity), `session` (heartbeat/leave/lease), `turns` (sequenced turn commands, acks), `budget` (per-link abuse limits), `systems` (connect/disconnect, arena, respawn queue), `announce` (manager HTTP), `look` (per-link `SetLook` stash, applied to the link's rider), `chat` (sanitize, label, broadcast), `probe` (local discovery replies), `web` (WebTransport listener, certificate rotation), `ranked` (join-ticket verification, result reporting), `elimination` (death log to rated events), `bot/` (ECS glue).
- **`client/` (lib + bin)** — desktop client (UDP) and browser client (wasm32, WebTransport; `platform.rs` holds the target splits, build with `web/build.ps1` or `web/build.sh`, design in `docs/specs/web.md`). `connection` (lifecycle, readiness, autoconnect, quit), `look` (chosen rider color/shape, sent to the server as `SetLook`), `input/` (keyboard queue, `TurnOutbox` redundant delivery), `menu`/`settings`/`browser`/`config` (UI, key bindings, room list, persisted preferences), `chat` (typing state, overlay), `account` (manager account, ranked tickets, rating/leaderboard), `local_scan` (finds local servers), `theme` (design tokens), `sim` (latency simulator), `render/` (arena, trails, riders, `shape` outlines, `smooth` render pose, `fx` particles, `layers`, camera, HUD, death, bot debug).
- **`lab/` (lib + bin)** — headless deterministic scenario runner over the real `simulate_players`. TOML scenarios in `crates/lab/scenarios/` (goldens `*.golden.jsonl`); JSONL trace + PNG output; `[match]` blocks play rounds (`shared::MatchRules`, last rider standing); `lab tournament <suite>` (suites in `crates/lab/suites/`) plays bot builds against each other and prints metric tables. **This is how an agent "plays" the game**: edit a scenario, re-run from tick 0, read trace and PNG. See `docs/guides/dev.md` § Lab.
- **`nettest/` (lib + tests)** — real server and client(s) in one process over loopback UDP with a link conditioner; scripted key presses, trace assertions (`tests/turns.rs`, `session.rs`, `chat.rs`, `look.rs`, `ownership.rs`, `web.rs`) and arrival measurements (`measure.rs`, `--ignored`). Uses wall-clock time, so a failure should be re-run alone before it is believed.
- **`server-manager/` (Bun/TypeScript)** — lobby service: room registry (in memory), anonymous accounts, join tickets, Elo ratings, leaderboard and placement (memory or Firestore store). API, env vars and deployment in its README; `bun run typecheck`.

## Netcode model (short version)

Forward-only authoritative server, immediate local prediction, client reconciliation only — no server rewind. Own rider is `Predicted`; opponents and bots are step-interpolated. A trail is one replicated+predicted `Trail(Vec<Vec2>)` of corners only, **not** child entities (an early plan used child entities per trail point): the head is `Position` and the tail is derived (`Trail::points`), so it is sent on turns, not every tick.

- **Turns** are sequenced commands (`TurnCommand { seq, direction, generation, tick }`), not Lightyear inputs: every undecided command is resent in each tick's unreliable `TurnBatch` (<= 16 commands, repeated for 48 ticks), the server decides each seq once, executes it at its intended tick, within a 12-tick late window, or rejects it with a reason, and acks cumulatively. Lightyear's native `PlayerInput` only feeds client prediction; the server overwrites it for humans.
- **Session:** the first message is a `SessionHello { protocol, name }`; the server admits (sanitized unique name) or rejects (`VersionMismatch`, `ServerFull`). Nothing replicates to a link before admission. Heartbeats, leave and respawn travel on ordered-reliable channels (`SessionChannel`, `RespawnChannel`); respawns are server-applied and tagged by `LifeGeneration`. The client sends `SetLook { color, shape }` on `SessionChannel` right after the hello; the server stashes the newest per link (`LinkLook`), applies it at spawn and live to the link's own rider. Chat (`ChatSend`/`ChatLine` on `ChatChannel`) is sanitized and labelled with the sender's name and color by the server, then broadcast.
- **Hardening:** per-link `LinkBudget`, capped unadmitted links, `PROTOCOL_ID` = hand-bumped `PROTOCOL_VERSION` + fingerprint of every sim constant (a physics retune changes it by itself; a *logic* change with the same constants needs a manual bump).

Implemented contract and measurements: `docs/specs/netcode-design.md`. Open items: `NETWORK.md`.

## Gameplay invariants (`docs/specs/core-gameplay-mechanics.md` is normative)

120 ticks/s, base speed 360 u/s, 2000x2000 arena, one turn per tick (each costs 5% of the cruise speed, floored at 0.8x), trail cap `TRAIL_MAX_LENGTH` (2400), rubber max 120 paid per unit of distance lost in the rubber zone (linear-ish in speed, ~22 ticks to die at base, ~14 at the cap, above the 12-tick late window) with regen proportional to the rubber left (slow when burned, fast when scratched) that also runs while grinding, an exponential wall approach inside a speed-wide zone (>= 12 units), capped inertial speed (`MAX_SPEED_MULT` 3.0) accelerated by grinding within `GRIND_RANGE` of any trail on either side, summed (own trail counts from `OWN_GRIND_MIN_FROM_HEAD` legs back; arena walls pay 0.4x) and fading exponentially, no crash bleed, deterministic spawn/safe-respawn search, manual human respawn only (free-for-all; in rounds mode, `GAME_MODE=rounds`, riders are placed at each round start and nobody respawns during a round, see `docs/specs/rounds-mode.md`). Sim arithmetic uses `+ - * /` only (no `powi`/`exp`) so client and server stay bit-identical.

## Architecture rules

Code style is enforced by `cargo fmt -- --check` and `cargo clippy --workspace --all-targets -- -D warnings` (workspace lints in `Cargo.toml`, test exemptions in `clippy.toml`, CI on PRs to `main` in `.github/workflows/lint.yml`); run both before committing. These rules are about where code goes and how it plugs into the sim:

- **Constants:** gameplay numbers are named consts in `shared/constants.rs` (or module-local next to their sole use), never inlined at call sites. Promote to a `Resource` only if it must vary at runtime.
- **Logging:** `warn!` = denied/degraded, `info!` = lifecycle (join/leave/spawn accepted), `debug!` = stale or per-tick noise. A binary installs its log subscriber before reading config.

- **Determinism:** the shared sim uses `f32`/`Vec2` only, headings as ±X/±Y swaps (no trig at turn sites), no `rand`, no wall clock, no unsorted `HashMap` iteration. Spawns come from a seed (`spawn_from_seed`, `find_safe_spawn`). Other randomness (bots, rotation) stays server-side outside the predicted state, or is seeded and replicated.
- **System order:** Bevy runs unchained systems in parallel. A new `FixedUpdate` system declares `.after(simulate_players)` / `.before(...)` with a comment saying why. Build the obstacle snapshot in one piece before any mutation.
- **Replication:** every replicated component gets one line in `protocol.rs` with its reason: `predict()` (client guesses and rolls back: position, rubber), `replicate_once()` (spawn-time: id, name), plain `replicate()` (player-changeable, live: color, shape), or local-only (`ActionState<PlayerInput>`). Cardinal turns and trail heads use discrete `step` interpolation.
- **Messages vs inputs:** never carry a one-shot action in `PlayerInput`; it is a Lightyear message. Every message a client can send freely gets a per-link budget in `server/budget.rs`.
- **Module homes:** `shared/` has no rendering, HTTP or `rand`. `server/` holds bot glue, respawn/auth and manager HTTP, and is the only place with secrets. `client/` holds render, UI, input and connection, and is the only place with `egui`.

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
- `NOTES.md` — open feature ideas (audio, spectator mode). `EXPLORATION.md` (Armagetron ideas), `BOT-AI-*.md` (bot research) — not committed work.

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