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
