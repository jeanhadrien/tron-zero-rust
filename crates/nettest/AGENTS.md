# `nettest/` — real server and clients over loopback

Root rules: `../../AGENTS.md`. Netcode contract and measurements: `docs/specs/netcode-design.md`.

## Purpose

Run the real server app and one or more real client apps in one process, over
loopback UDP with a link conditioner, and assert on what the client sees. It is
the only test of the netcode end to end.

## Boundaries

- Depends on `server/` and `client/` libs (and `shared/`). Test code only: no
  game logic and no helper that the apps themselves would need.
- Assert invariants, not exact numbers: it uses wall-clock time and the
  conditioner is unseeded.

## Map

- `src/server.rs`, `src/client.rs` — app harnesses, scripted key presses. `src/analysis.rs` — trace helpers.
- `tests/turns.rs`, `session.rs`, `chat.rs`, `look.rs`, `ownership.rs`, `web.rs`, `rounds.rs` — behaviour.
- `tests/measure.rs` — arrival and bandwidth measurements (`--ignored --nocapture`).

## Rules

- `cargo nextest run -p tron-zero-nettest` takes about two minutes and binds
  real UDP ports. Run it once, alone, at the end of a change that a client can
  observe (server, client, protocol).
- Never run two copies at once, including from parallel agents or worktrees:
  overlapping runs fail spuriously. One agent owns it at a time.
- A failure is re-run alone before it is believed.
- A new client-observable behaviour gets one test that plays through it with a
  real client, not a unit test of both ends.
