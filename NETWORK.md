# Netcode and operations backlog

Status: updated 2026-11-04 (each item re-checked against the code). This file
lists only what is **open**. What is implemented, how it works and the measured
numbers are in `docs/specs/netcode-design.md`; the headless harness is
`crates/nettest` (`cargo test -p tron-zero-nettest`; arrival measurements with
`cargo test -p tron-zero-nettest --test measure -- --ignored --nocapture`).

## Needs a decision from JH

- [ ] **Approve the tuning values.** Nothing below has been signed off by a
  human playtest: the physics retune (`MAX_SPEED_MULT` 2.0, `SLIDE_DISTANCE`
  24, `ARENA_WALL_GRIND` 0.4, inertia rates, rubber drain/regen, arena
  2000x2000, trail cap 2400), the 12-tick late window, the redundancy bounds
  (16 commands, 48 ticks) and the abuse budgets (600 batches/s, 60 rejections/s,
  10 requests/s). The netcode numbers are measured on loopback with a
  conditioner but untuned against real players.
- [ ] **Turn-delay design question.** The client uses no extra input delay
  (`InputDelayConfig::no_input_delay()`); a late turn (inside the window)
  costs one visible state rollback. Options: keep it, add a small fixed delay
  so fewer turns are late, or have the client patch its input history from the
  ack's `Applied { server_tick }` to avoid the snap. Needs real-player data.

## Open: transport and loss

- [ ] **Unauthenticated, unencrypted raw UDP (desktop path).** Any address that
  sends a packet becomes a link; spoofed-source floods are only bounded (32
  unadmitted links, 5 s hello timeout, no replication, one tiny ack per frame).
  Planned: Lightyear's netcode transport with connect tokens issued by the
  manager. The browser path already runs over WebTransport. Server authority is
  not a substitute for authenticated sessions.
- [ ] **Burst loss and outages over 400 ms.** A command is repeated for at
  most 48 ticks (400 ms) and the outbox holds 16, so a longer outage loses
  turns (rejected or never seen, never invented). The conditioner drops
  packets independently; bursts of consecutive loss, duplication and long
  outages are not measured.
- [ ] **Natural timeline resync under jitter.** A forced resync with turns in
  flight is covered by a nettest; a resync caused by real jitter has not been
  observed end to end.
- [ ] Not measured: client frame hitches and catch-up bursts, long sessions,
  server restart, two or more clients playing together under impairment, abuse
  budgets against real client traces.
- [ ] Keyboard queue behaviour on focus loss and long sync stalls (menu,
  death, respawn, disconnect/reconnect are verified).
- [ ] The conditioner is unseeded: runs are not reproducible and no
  seeds/traces are saved, so failures cannot yet become regression fixtures.

## Open: gameplay and fairness

- [ ] **Bot self-trapping.** 22 of the 23 deaths in the `bot_arena` and
  `bot_arena8` goldens are `own_trail`; the brain boxes itself in. Tuning the
  strategies changes those goldens (bless after intent).
- [ ] Collision fairness follow-ups (`crates/shared/src/systems/player.rs`):
  same-tick crossings, head-on meetings and endpoint tolerances are resolved.
  Left: client prediction reads an opponent's newest confirmed trail, about
  one RTT behind the predicted tick, so the own rider can locally drive
  through that stretch until corrected (extrapolating the opponent head is
  the candidate fix); x86_64/wasm32 bit-identity is argued (scalar `+ - * /`,
  correctly rounded sqrt, no FMA), not measured (run the lab goldens on
  wasm32-wasip1 to verify).
- [ ] Diagnostics: structured tracing correlating session, command, intended
  tick, applied/rejected and rollback range (the server `TurnLog` and harness
  traces exist; client side and rollback ranges do not); RTT/jitter/loss,
  correction and bandwidth metrics.
- [ ] Presentation and capacity: render smoothing exists (see
  `docs/specs/netcode-design.md`), but its feel under real jitter and loss is
  unmeasured and opponent trail tails still step at snapshot rate. Still to do:
  profile `Trail` replication size, rollback cost and brute-force collision
  scans at the intended player count; server tick-time percentiles against the
  8.33 ms budget; declared player cap, bandwidth and correction targets.

## Open: UI, deployment and operations

- [ ] **In-game UI never run by a human.** Menu, settings/rebinding, server
  browser, HUD, death overlay, quit and autoconnect are covered by unit tests
  and nettest only. The manual checks in `docs/guides/dev.md` still need a person at
  the window.
- [ ] Deployment recipes exist (`docs/guides/release.md`, `deploy/gce/`), but
  diagnostics collection and operational limits are not documented for public
  hosting.

## Candidate tooling (not adopted)

Linux `tc netem` and Windows clumsy/WinDivert for real-socket impairment,
Wireshark for packet shape, `proptest` for command sequences, and a CI
workflow running the bounded nettest matrix. A broader scheduled matrix
(RTT 30-250 ms, jitter 0-50 ms, loss 0-5%, outages 100-500 ms, 30-144 FPS)
should be a small representative set, not the Cartesian product.
