# `bots/` — the bot brain

Root rules: `../../AGENTS.md`. Design intent and settled decisions: `BOT-AI-SPECS.md`.

## Purpose

Decide a bot's steering input from a read-only `WorldSnapshot`. One planner
serves every bot. Used by the server (real games) and the lab (measurement).

## Boundaries

- Depends on `shared/` only. It never knows which runner calls it: no server
  or lab types, no env, no logging of game events.
- Deterministic: no `rand`, no wall clock, no unsorted `HashMap` iteration that
  affects a decision. Callers supply seeds.
- Bots act only through ordinary `PlayerInput`s. Never touch physics or sim state.
- Bot logic that both runners need goes here (for example `drive`), not copied
  into `server/` and `lab/`.

## Model

- **Difficulty = budget** (`config::BotKnobs`): reaction, horizon, variety, noise.
  A weak bot misjudges distances and room; it never acts at random.
- **Personality = weight vector** (`objective::Weights`) over named terms.
  A new goal is one field in `Terms`, one in `Weights`, one line in the planner.
- **A bot = `BotBuild`** (name, personality, knobs, weights, tuning). Rosters
  are TOML files: one `[tuning]` (`tuning::Tuning`, the planner's global
  constants, every key required) shared by their `[[build]]`s;
  `rosters/default.toml` is the game's roster, embedded by `roster::builtin`
  (one medium bot, four hard bots of different personalities, the artist
  Escher). `rosters/ladder.toml` is the generated five-rung difficulty ladder.
- A tuned number of the planner lives in `Tuning`, never as a `const`. Consts
  stay for structure only: grid and buffer sizes, `MAX_TURNS`, the reply set,
  the artist's cycles, and the lab's authoring curve and personality presets.

## Map

- `planner.rs` — candidates (short maneuvers), scoring, fight terms, the pick. `plan.rs` — maneuvers and the plan being followed.
- `objective.rs` — `Terms`, `Weights`, round heat. `config.rs` — knobs, personalities, the `difficulty = N` authoring curve.
- `space.rs` — timed flood fill of reachable room. `race.rs` — who reaches which territory first. `grind.rs` — speed earned beside trails and walls.
- `forecast.rs` — the best candidates verified with the real `step_riders`. `style.rs` — the artist's figure.
- `drive.rs` — one tick of decisions for a set of brains. `schedule.rs` — decision cadence and level-of-detail `Tier` (thresholds and limits in `tuning.rs`). `noise.rs` — seeded misjudgment. `world.rs` — the snapshot.

## Known limits (do not rediscover)

- Grids assume the default 2000-unit arena.
- Between evading riders kills are rare; rounds end mostly through the round
  heat making bots careless. A server-driven steering is the intended replacement.
- The rollout radius assumes about 2x speed while the cap is 3x.
- At 5+ riders the `Tier` cuts horizon and variety; the ladder was measured at full detail.

## Test and measure

- `cargo nextest run -p tron-zero-bots` for unit logic.
- Behaviour is measured in the lab, not asserted here: `lab tournament <suite> --quick`
  and the gates in `crates/lab/tests/gates.rs`. See `../lab/AGENTS.md`.
- Change a weight or constant for a measured reason, check the figure it should
  move, and stop after three attempts without a clear gain.
- Changing knobs or weights of a roster build, or the roster's `[tuning]`, changes its rating id.
