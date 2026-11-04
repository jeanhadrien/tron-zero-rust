# Netcode and operations backlog

Status: updated 2026-11-02. This file lists only what is **open**. What is
implemented, how it works and the measured numbers are in
`docs/netcode-design.md`; the headless harness is `crates/nettest`
(`cargo test -p tron-zero-nettest`; arrival measurements with
`cargo test -p tron-zero-nettest --test measure -- --ignored --nocapture`).

Done and verified, for orientation (details in the design doc): one-shot turn
delivery (sequenced commands, redundant per-tick batches, intended-tick
execution with a 12-tick late window, coalesced acks), hello handshake with
protocol fingerprint and capacity, no replication before admission, per-link
abuse budgets, rejection delivery under loss, autoconnect/quit races, and the
announcer/manager token flow.

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

- [ ] **Unauthenticated, unencrypted raw UDP.** Any address that sends a
  packet becomes a link; spoofed-source floods are only bounded (32 unadmitted
  links, 5 s hello timeout, no replication, one tiny ack per frame). Planned:
  switch to Lightyear's netcode transport with connect tokens (issued by the
  manager) and then WebTransport for browsers. Server authority is not a
  substitute for authenticated sessions.
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

- [ ] **Bot self-trapping.** About 95% of bot deaths in `bot_arena` traces are
  `own_trail`; the brain boxes itself in. Tuning the strategies changes the
  `bot_arena` goldens (bless after intent).
- [ ] Collision fairness (`crates/shared/src/systems/player.rs`): simultaneous
  newly grown trail crossings and head-to-head movement are not swept against
  each other; behaviour with missing/stale opponent history is "omit that
  opponent"; equal-tick ties, near-contact tolerances and cross-platform float
  determinism are unverified.
- [ ] Diagnostics: structured tracing correlating session, command, intended
  tick, applied/rejected and rollback range (the server `TurnLog` and harness
  traces exist; client side and rollback ranges do not); RTT/jitter/loss,
  correction and bandwidth metrics.
- [ ] Presentation and capacity: (render smoothing exists: see
  `docs/netcode-design.md`; its feel under real jitter and loss is unmeasured;
  opponent trail tails still step at snapshot rate); profile `Trail` replication size, rollback cost and brute-force
  collision scans at the intended player count; server tick-time percentiles
  against the 8.33 ms budget; declared player cap, bandwidth and correction
  targets.

## Open: UI, deployment and operations

- [ ] **In-game UI never run by a human.** Menu, settings/rebinding, server
  browser, HUD, death overlay, quit and autoconnect are covered by unit tests
  and nettest only. The manual checks in `docs/dev.md` still need a person at
  the window.
- [x] **Linux SIGTERM unregister** verified on the GCE container
  (`systemctl stop` logs "Unregistered from manager"; the room disappears).
- [x] **Manager image built and deployed** to Cloud Run (2026-11-02).
- [ ] Document the supported deployment topology (UDP firewall, diagnostics
  collection, operational limits) before public hosting; today only the
  friends-on-my-PC recipe exists (`docs/release.md`).
- [ ] WASM client: not wired. Room listing works in a browser, Connect is
  disabled there until WebTransport exists.

## Candidate tooling (not adopted)

Linux `tc netem` and Windows clumsy/WinDivert for real-socket impairment,
Wireshark for packet shape, `proptest` for command sequences, and a CI
workflow running the bounded nettest matrix. A broader scheduled matrix
(RTT 30-250 ms, jitter 0-50 ms, loss 0-5%, outages 100-500 ms, 30-144 FPS)
should be a small representative set, not the Cartesian product.
