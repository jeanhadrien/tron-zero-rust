# `shared/` — rules of the game and wire protocol

Root rules: `../../AGENTS.md`. Gameplay rules: `docs/specs/core-gameplay-mechanics.md` (normative).

## Purpose

Everything that holds whoever runs the game: the simulation, the rules around
it, and what goes on the wire. The server, the client, the lab and the bots all
build on it.

## Boundaries

- Depends on nobody in this workspace. It must not mention the server, the
  client or the lab, even in comments or names (network-role words of the wire
  format, such as `SimulationRole::Server`, are fine).
- No rendering, no HTTP, no `rand`, no wall clock, no file or env access.
- A piece belongs here when it is a rule of the game (rider reset, spawn
  search, match flow, death attribution). How one runner drives or observes the
  game (env vars, replication targets, traces, metrics, seeds) stays in that runner.

## Map

- `systems/` — `simulate_players` at 120 Hz (`TICK_HZ`): turn → sense → move → rubber. The order is load-bearing.
- `physics.rs` — `step_rider`, pure. `constants.rs` — every gameplay number.
- `components/` — incl. `trail.rs` (the rollbackable trail of corners) and `round.rs` (replicated `MatchStatus`).
- `spawn.rs` — safe-spawn search, `SpawnField` (spawn inputs), `placed` (the fresh-life bundle). `math.rs` — rotations, arena walls, seeded spawn.
- `match_rules.rs` — `MatchRules` (respawn, end condition, rounds) and `MatchFlow`, the pure round state machine. Outside the sim: not read by `simulate_players`, not in `PROTOCOL_ID`.
- `elimination.rs` — rated elimination events from the death log.
- `protocol.rs` — components, messages, channels, `PROTOCOL_ID`, turn constants. `probe.rs` — UDP discovery datagram. `polyline.rs` — render-only helpers.

## Rules

- **Determinism:** `f32`/`Vec2` only, headings as ±X/±Y swaps (no trig at turn sites), arithmetic `+ - * /` only (no `powi`/`exp`), no unsorted `HashMap` iteration. Client and server must stay bit-identical.
- **Constants:** gameplay numbers are named consts in `constants.rs` (or module-local next to their sole use). Promote to a `Resource` only if it must vary at runtime.
- **System order:** a new `FixedUpdate` system declares `.after(simulate_players)` / `.before(...)` with a comment saying why. Build the obstacle snapshot in one piece before any mutation.
- **Replication:** every replicated component gets one line in `protocol.rs` with its reason: `predict()` (position, rubber), `replicate_once()` (id, name), plain `replicate()` (color, shape), or local-only (`ActionState<PlayerInput>`). Cardinal turns and trail heads use discrete `step` interpolation.
- **Messages vs inputs:** a one-shot action is a Lightyear message, never a `PlayerInput`.
- **Protocol id:** `PROTOCOL_ID` = hand-bumped `PROTOCOL_VERSION` + fingerprint of every sim constant. A constant change bumps it by itself; a logic change with the same constants, or a change to a replicated type, needs a manual bump and a re-stamp of the lab goldens.

## Gameplay invariants (summary)

120 ticks/s, base speed 360 u/s, 2000x2000 arena, one turn per tick (each costs
5% of cruise speed, floored at 0.8x), trail cap `TRAIL_MAX_LENGTH` (2400),
rubber max 120 (about 22 ticks to die at base speed, 14 at the cap, above the
12-tick late window), speed capped at `MAX_SPEED_MULT` 3.0 and earned by
grinding within `GRIND_RANGE` of any trail (own trail from
`OWN_GRIND_MIN_FROM_HEAD` legs back; arena walls pay 0.4x), deterministic
spawn search. Free-for-all: manual human respawn. Rounds mode: riders are
placed at each round start and nobody respawns during a round.

## Wire model (summary; contract in `docs/specs/netcode-design.md`)

Forward-only authoritative server, immediate local prediction, client
reconciliation only. A trail is one `Trail(Vec<Vec2>)` of corners, sent on
turns. Turns are sequenced `TurnCommand`s resent in each tick's `TurnBatch`
(<= 16 commands, 48 ticks), decided once by the server within a 12-tick late
window. The first message of a link is `SessionHello`; nothing replicates
before admission.

## Test

`cargo nextest run -p tron-zero-shared` (unit, property, rollback; fast).
A sim change usually changes lab goldens: see `../lab/AGENTS.md`.
