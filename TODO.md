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
