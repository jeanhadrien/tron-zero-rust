# Rounds mode (last rider standing)

Status: implemented 2026-11. Normative for the server's rounds mode. The
free-for-all (the default, see `core-gameplay-mechanics.md` section 6) is
unchanged. Code: `crates/server/src/rounds.rs`, rules type
`shared::MatchRules` (`crates/shared/src/match_rules.rs`, also played by the
lab), replicated state `shared::MatchStatus`, client overlay
`crates/client/src/render/round.rs`.

## Selecting the mode

The server reads its environment at start; an invalid value stops it (exit 2).

| Variable | Default | Meaning |
|----------|---------|---------|
| `GAME_MODE` | `ffa` | `ffa` (free-for-all) or `rounds`. |
| `ROUNDS` | 5 | Rounds per match, at least 1. |
| `ROUND_SECS` | 90 | Round time limit in seconds; `0` means no limit. |
| `INTERMISSION_SECS` | 3 | Break between rounds and before round 1, at least 1. |
| `FINAL_INTERMISSION_SECS` | 8 | Break after the last round (standings), at least 1. |

`scripts/play-local.bat` and `.sh` start the server with `GAME_MODE=rounds`.

## Rules

- **Riders** are everybody in the arena roster: admitted humans and bots, alive
  or waiting. A match needs at least **two riders**. With fewer, the server is in
  phase `Free`: free-for-all behaviour (initial spawn, manual respawn with
  Space, bots respawn and cycle as usual).
- **Phases** (`MatchPhase`): `Free` -> `Starting` (countdown of one
  intermission, every rider stopped) -> `Playing` (round 1) -> `Intermission`
  -> `Playing` (round 2) ... -> last round -> `MatchOver` (final intermission,
  standings) -> `Playing` round 1 of a new match with scores reset. If fewer
  than two riders remain during any break, the match is dropped and the phase
  is `Free` again; a second rider then starts a new match.
- **Round start:** every rider (also those that joined during the previous
  round) is placed with the deterministic safe-spawn search on an arena without
  trails, alive, at base speed and full rubber. Humans enter a new life
  (`LifeGeneration` is incremented), so commands of the previous round are stale.
- **During a round** nobody respawns. A manual respawn request is answered
  `RespawnOutcome::WaitForNextRound`; bots are not respawned or replaced
  (bot auto-cycle is paused). A rider that joins during a round (or a break) is
  admitted dead and placed at the next round start. A rider that leaves counts
  as eliminated.
- **Round end:** after the tick's deaths, when at most one rider is alive, or at
  the time limit (`MatchRules::round_end`). The winner is the last rider alive:
  nobody wins a time-out with several alive, or when the last riders die on the
  same tick. The winner scores one round win. The surviving riders are stopped
  (dead, trail cleared) so nothing happens during the break and no death of the
  break reaches the ranked reports.
- **Scores** are round wins per rider, kept for the match only, and reset when
  a new match begins. A rider that leaves takes its score with it.
- **Ranked:** unchanged. Eliminations are resolved from the death log of the
  tick exactly as in free-for-all; rounds mode has no rating of its own.

## Wire

Server-authoritative display state; it does not enter the predicted simulation.

- A singleton entity with `MatchStatus` (`replicate()`, no prediction), spawned
  by the server only in rounds mode (its absence means free-for-all):
  `rounds`, `round` (from 1, 0 when `Free`), `phase`, `secs_left` (whole seconds
  left on the current clock, round limit or break, once per second), `winner`
  (name of the last round's winner, `None` for a draw) and `standings`
  (name, wins; most wins first).
- `RespawnOutcome::WaitForNextRound` is the refusal reason of a respawn request.
- `PROTOCOL_VERSION` 11.

## Client

- Top bar: `ROUND x / y`, the clock when a limit runs, the best scores; in
  phase `Free` of a rounds server: `WAITING FOR OPPONENTS`.
- Banner in the middle during `Starting`, `Intermission` and `MatchOver`: round
  winner (or no winner), countdown, and the standings list after the last round.
- Death panel while a round is played: `OUT OF THE ROUND` and "Waiting for the
  next round..."; no Space prompt. Between rounds the banner replaces it.
