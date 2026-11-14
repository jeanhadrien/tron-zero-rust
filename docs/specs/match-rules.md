# Match rules (rounds, respawns, end conditions)

Status: implemented 2026-11. Normative for how every server plays. There are no
game modes: a server plays **matches** of one or more **rounds** by the rules of
its config file. "Free-for-all" is one long round with respawns; "rounds" is
several last-standing rounds. Code: rules and state machine
`shared::MatchRules` / `shared::MatchFlow` (`crates/shared/src/match_rules.rs`,
also played by the lab), server runner `crates/server/src/game.rs`, replicated
state `shared::MatchStatus` and `shared::Score`, client overlays
`crates/client/src/render/round.rs` and `scoreboard.rs`.

## Config

The `[match]` and `[round]` tables of the server config file
(`docs/guides/building.md`); an invalid value stops the server (exit 2).

| Key | Meaning |
|-----|---------|
| `match.rounds` | Rounds per match, at least 1. |
| `match.intermission_secs` | Countdown before round 1 and break between rounds, at least 1. |
| `match.end_screen_secs` | Break after the last round (scores shown), at least 1. |
| `round.secs` | Round time limit in seconds; `0` means no limit. |
| `round.respawn` | `anytime` (a dead rider comes back during the round) or `next_round`. |
| `round.last_standing` | End the round when at most one rider is alive. Needs `next_round`. |
| `round.end_without_humans` | Optional, `true` by default. End the round when no human is left in it. |
| `round.end_without_humans_secs` | Optional, `4` by default. Seconds the bots keep riding after the last human is out, before the round ends; `0` ends it at once. |

A round may have no end condition (it lasts until the match is dropped), but
only with `anytime` respawns: with `next_round` and no end condition the dead
would never come back, so the config is refused.

Shipped configs (`crates/server/config/`): `official.toml`, `dev-cloud.toml`
and `local.toml` (`scripts/play-local.*`) all play five 90 s last-standing
rounds.

## Rules

- **Riders** are everybody in the arena roster: admitted humans and bots, alive
  or waiting.
- **Start condition** (fixed, not in the config): at least one human connected,
  and at least two riders when `last_standing` is on (one rider would win at
  once). Until then the phase is `Free`.
- **`Free` (waiting for players):** humans ride free: initial spawn, manual
  respawn with Space. Bots do not ride: they are stopped and not respawned, so
  bots never play each other on an empty server. A human is not placed on the
  tick the match can start (it would be stopped at once by the countdown).
- **Phases** (`MatchPhase`): `Free` -> `Starting` (countdown of one
  intermission, every rider stopped) -> `Playing` (round 1) -> `Intermission`
  -> `Playing` (round 2) ... -> last round -> `MatchOver` (end screen) ->
  `Playing` round 1 of a new match with scores reset. When no human is
  connected any more, in any phase, the match is dropped and the phase is
  `Free`. If too few riders remain during a break, the match is dropped too.
- **Round start:** every rider (also those that joined during the previous
  round or break) is placed with the deterministic safe-spawn search on an
  arena without trails, alive, at base speed and full rubber. Humans enter a
  new life (`LifeGeneration` is incremented), so commands of the previous round
  are stale. A bot gets a fresh brain (no plan, no target, its engagement clock
  at zero): the bots' wish to engage grows with the age of the brain, so a
  round starts calm.
- **Respawns during a round:** with `anytime`, a dead human respawns when they
  press Space and a dead bot after its delay (240 ticks); a human who joins
  mid-round is placed at once. With `next_round`, nobody respawns: a request is
  answered `RespawnOutcome::WaitForNextRound`, and a rider that joins is
  admitted dead and placed at the next round start ("waiting for spawn").
  Between rounds nobody is placed.
- **Round end** (`MatchRules::round_end`), after the tick's deaths, the first
  condition met:
  - `last_standing`: at most one rider alive. The survivor wins; nobody wins
    when the last riders die on the same tick.
  - `end_without_humans`: every human has been dead for
    `end_without_humans_secs` with `next_round` respawns (with `anytime` a
    dead human can come back, so it never fires). Meanwhile the bots ride on
    and may still win by last standing; at the end, with one rider alive, last
    standing names it the winner first; with more, nobody wins.
  - Time limit. In a one-round match the winner is the top scorer: most kills,
    then fewest deaths; without a kill, or with a tie at the top, nobody wins.
    With several rounds the scores span the match, so a time-out has no winner.
    The decision is `shared::round_winner`; the lab does not keep scores and
    leaves a timed-out round without a winner (no lab metric reads it).

  The winner scores one round win. The surviving riders are stopped (dead,
  trail cleared) so nothing happens during the break and no death of the break
  reaches the ranked reports.
- **Scores** (`Score`): kills, deaths and round wins per rider, kept for the
  match and reset when a new match begins. A death on another rider's trail is
  a death for the victim and a kill for the trail's owner; in a mutual crash
  both get one of each; a wall or one's own trail is a death only. A rider that
  leaves takes its score with it; a human who comes back starts at zero.
- **Ranked:** unchanged. Eliminations are resolved from the death log of the
  tick; matches have no rating of their own.

## Wire

Server-authoritative display state; it does not enter the predicted simulation.

- A singleton entity with `MatchStatus` (`replicate()`, no prediction), always
  present: `rounds`, `round` (from 1, 0 when `Free`), `phase`, `secs_left`
  (whole seconds left on the current clock, round limit or break, once per
  second), `winner` (name of the last round's winner, `None` for a draw) and
  `respawn_now` (whether the server accepts a dead human's respawn request
  right now: the client shows the Space prompt from it and never re-derives it).
- `Score` on each rider (`replicate()`), and `RoundTrip` on each human: the
  link's round-trip time in ms, measured by the server, updated once per second.
- `RespawnOutcome::WaitForNextRound` is the refusal reason of a respawn request.
- `PROTOCOL_VERSION` 12.

## Client

- Top bar: `ROUND x / y` when a match has several rounds, and the clock when a
  limit runs; `WAITING FOR PLAYERS` in phase `Free`.
- Banner during `Starting`, `Intermission` and `MatchOver`: the round (or, for a
  one-round match, the match) winner or no winner, and the countdown.
- Scoreboard while Tab is held, and on the end screen: name, kills, deaths,
  wins (several rounds only), ping.
- Death panel: the Space prompt when a respawn is allowed now; otherwise
  `OUT OF THE ROUND` and "Waiting for the next round...". Between rounds the
  banner replaces it.
