# TODO

Free-to-edit list of things to do or keep in mind. Agents may write here.

- One section per scope or task, each a list of bullets.
- Every bullet starts with a status:
  - `AGREED:` JH confirmed it, to do later.
  - `AGENT:` an agent judged it worth remembering. Not confirmed.
- Remove a bullet when it is done or dropped. Promote `AGENT:` to `AGREED:` only when JH says so.

## Bots: design rules to keep

- AGENT: Difficulty is limiting the bot (reaction, horizon, inference, noise), never fudging outcomes. The sim is deterministic and a bot sees everything, so every extra capability is a deliberate budget item.
- AGENT: Inference (predicting an opponent's turn) is the most unfair-feeling lever. Keep it an explicit budget item.
- AGENT: Perception, if added, is defined in grid units, not the camera view. Zooming out must not give bots more.
- AGENT: Every knob is a surface to validate and balance. Decide which are player-facing, dev-only or derived before adding one.

## Bots: ideas not built

- AGENT: Tells (visible wind-up before a move). They conflict with optimal play, so disable them above a difficulty threshold.
- AGENT: Mood layer: a transient state on top of the base personality that decays back (grudges, reactions).
- AGENT: Habit tracking: model one opponent's repeated patterns to bait them. Budget item, not for low tiers.
- AGENT: Coalitions. Floor: do not cut an ally. Ceiling: coordinated pincers. Needs an anti-dogpile bias so bots do not all target the human.
- AGENT: Chat as templated narration of the planner's intent ("Cutting left."). Bots may lie. Bots stay tagged as bots by name.
- AGENT: Decaying occupancy memory of seen trails, as a difficulty knob.
- AGENT: Account for the bot's own input delay (reaction + ping) in the planner.
- AGENT: Spawn behavior: hunt on respawn, grace window or leave spawn. Must not insta-kill fresh spawns.
- AGENT: FFA target selection: nearest, most dangerous, random or coalition-aware.
- AGENT: Artist bot: take free kills (no detour, no added risk); nested figures at several scales.
- AGENT: Symmetry breaking for mirror matches (identical bots on symmetric spawns can loop forever).
- AGENT: Explicit list of intended degeneracies versus bugs; anti-clump and anti-suicide smoke checks.
- AGENT: Adaptive difficulty from the player's win rate (may feel like rubber-banding).
- AGENT: Persistent bot identity across matches (stats, grudges).
- AGENT: Offline self-play to evolve personality weights.

## Bots: CPU budget

- AGENT: The load tiers (`[tuning.tiers]`) and the search budget (`verify_top`, `race_top`, `reply_top`) are CPU budgets, yet they sit in the roster's tuning, hashed into every bot's rating id.
- AGENT: Extract them into one budget piece: a cost model per decision and a per-tick budget the server owns (it knows its hardware), separate from what defines a bot.
- AGENT: Open question: a budget that changes with the machine makes the same rated bot play differently across servers. Decide whether ratings should care.

## Hosted server (sidecar), from the code review

Code: `crates/server/src/embedded.rs`, `crates/client/src/host.rs`.

- AGENT: Error handler. `embedded.rs` sets `set_error_handler(warn)`, so ECS errors in the hosted server only log and the match runs on in a broken state (the server binary keeps the default). Fix: keep the default handler, detect the bind failure by the port pre-check or `catch_unwind` → `Failed`. Alternative: keep `warn` and show "server degraded" in the menu.
- AGENT: Executor. `SingleThreadedExecutor` is set only on schedules that exist at build time; later ones may keep the multi-threaded executor and compete with the client's task pool. Unverified. Fix: list the schedules' executors after the first `update()` and set the rest. Alternative: build without the `multi_threaded` bevy feature for the server.
- AGENT: Probe reach. The busy-port probe only reaches `127.0.0.1`; a server bound to one LAN IP on the same port is missed, and on Windows `bind(0.0.0.0:p)` can succeed next to it. Fix: document the limit and require our own probe reply after the bind before reporting `Running`. Alternative: probe every local interface and bind exclusively (`SO_EXCLUSIVEADDRUSE`).
- AGENT: LAN exposure. "Host a server" opens an unauthenticated, unencrypted UDP server on all interfaces with no sign in the UI (the bind itself is JH's decision). Fix: show "Visible to your network" and the address next to `Running`. Alternative: a "LAN" toggle, default `127.0.0.1`, `0.0.0.0` only when on.

## Hosted server: next steps

- AGENT: Measure the cost of embedding the server: client binary size, client build time, frame time with the server and bots running. Check that the client's `multi_threaded` and the server's `webtransport` features don't leak into each other.
- AGENT: Server lifetime independent of the client session (it keeps running when the client leaves its game). Today it stops when the client exits.
- AGENT: Tag the hosted server's row "yours" in the local server list.
- AGENT: `TRON_HOST=1` autostart in `just play`, to replace the server window.
- AGENT: Web: sidecar on the client's own thread (feature split `net`, in-memory IO, one `update()` per frame). Measure bot cost first. Web Worker only if frame time suffers.
- AGENT: Game mode presets in the host menu.
- AGENT: Tutorial / play-vs-bots on top of the hosted server (scripted scenarios may reuse the lab's `Scenario`).
- AGENT: Notice to remote players when the host stops (needs a protocol bump). Today they time out after `SESSION_TIMEOUT_SECS`.

## Audio (not implemented)

- AGENT: Design proposal (generative, synthesized from sim state): `docs/specs/audio.md`.
- AGENT: Engine hum scaled with the rider's speed multiplier; explosion on death.
- AGENT: Spatial listener on the local rider, so opponents' sounds pan and fade with distance.
- AGENT: Volume setting in the settings screen (persisted in `client.json`).
- AGENT: Client-only: no sim or protocol change. Keep it out of `shared/` and the server.
- AGENT: The `bevy` feature set is trimmed (no audio compiled in, see `docs/guides/building.md`). Enabling it grows the wasm size, so measure first.

## Spectator mode (not implemented)

- AGENT: After death, follow another rider instead of freezing on the dead one (cycle with a key).
- AGENT: Camera work only, if the client already receives every rider's state. Check what replication and interpolation give a dead player before assuming so.
- AGENT: Must not interfere with manual respawn: Space still requests it, and the death overlay stays.
- AGENT: A true spectator that joins without a rider needs a protocol change (admission without spawning, capacity rules). Decide whether it is wanted.
