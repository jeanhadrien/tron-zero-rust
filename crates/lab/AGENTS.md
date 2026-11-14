# `lab/` — headless runner for scenarios and bot measurement

Root rules: `../../AGENTS.md`. Usage and metric definitions: `docs/guides/dev.md` § Lab.

## Purpose

Run the real `simulate_players` without a window or a network, deterministically,
and observe it. This is how an agent "plays" the game: edit a scenario, re-run
from tick 0, read the trace and the PNG. It also measures bots.

## Boundaries

- Depends on `shared/` and `bots/`. Never on `server/` or `client/`.
- The lab observes; it does not define the game. Never re-implement here
  something the server also does (round flow, rider reset, spawn inputs, bot
  driving): call the shared code, or move the logic down to `shared/` / `bots/`.
- Lab-only by nature and staying here: traces, metrics, forced-kill
  attribution, pattern score, tournaments, seeds, TOML file formats.
- Wall clock is allowed only for the decision-cost timing, reported apart from
  the deterministic output. `rayon` results must not depend on the thread count.

## Commands (prefix with `cargo run --release -p tron-zero-lab --`)

- `<scenario.toml> [--seed N] [--png P] [--frames DIR] [--events P]` — one scenario: JSONL trace, PNG, summary.
- `tournament <suite.toml> --quick` — builds against each other, metric tables per build. `--quick` is the normal way to measure; full seed counts are for a rare final check.
- `tune <file>` — one-parameter-at-a-time sweep of knobs and weights by name; flags dead knobs.
- `ladder <file>` — degrade a build into difficulty rungs, write a roster file.
- Every command has `--help`. Tournament, tune and ladder use half the cores unless `--threads N`.

## Map

- `sim.rs` (the `Lab`), `scenario.rs` (TOML), `render.rs` (PNG), `builds.rs` (builds by name, shared by suites, tune and ladder files).
- `tournament.rs`, `bench.rs` (a candidate against a pool), `tune.rs`, `ladder.rs`.
- `stats.rs`, `forced.rs` (forced kills), `pattern.rs` (pattern score).
- Data: `scenarios/` (+ `*.golden.jsonl`), `puzzles/` (pass/fail bot situations), `suites/`, `tuning/`.

## Tests

- `cargo nextest run -p tron-zero-lab` (about 40 s): mechanics, golden traces, and the bot gates (`tests/gates.rs`, `tests/roster.rs`) with loose thresholds.
- An intended change of behaviour re-blesses goldens: `LAB_BLESS=1` (add `LAB_BLESS_NO_BUMP=1` when the protocol version did not change). Review the golden diff.
- A gate threshold is loosened only with a measured before/after in the commit message.

## Pitfalls

- `scenarios/goldens.protocol` must stay LF. On Windows a CRLF copy makes the stamp test fail until a re-bless.
- Compare variants on the same seeds; chase big differences, not small ones.
- Never leave a run in the background and stop; never take all cores.
- Forced-kill attribution and the bot grids assume the default 2000-unit arena.
