# Notes

Open feature ideas, not committed work. None exists in the code today.

## Bot CPU budget

- The load tiers (`[tuning.tiers]`) and the search budget (`verify_top`, `race_top`, `reply_top`) are CPU budgets, yet they sit in the roster's tuning, hashed into every bot's rating id.
- Extract them into one budget piece: a cost model per decision and a per-tick budget the server owns (it knows its hardware), separate from what defines a bot.
- Open: a budget that changes with the machine makes the same rated bot play differently across servers; decide whether ratings should care.

## Audio

Design proposal (generative, synthesized from sim state): `docs/specs/audio.md`.

- Engine hum scaled with the rider's speed multiplier; explosion on death.
- Spatial listener on the local rider, so opponents' sounds pan and fade with distance.
- Volume setting in the settings screen (persisted in `client.json`).
- Client-only: no sim or protocol change. Keep it out of `shared/` and the server.
- The `bevy` feature set is trimmed (no audio compiled in, see `docs/guides/building.md`): enabling it grows the wasm size, so measure first.

## Spectator mode

- After death, follow another rider instead of freezing on the dead one (cycle with a key).
- Camera work only, if the client already receives every rider's state (check what replication and interpolation give a dead player before assuming so).
- Must not interfere with manual respawn: Space still requests it, and the death overlay stays.
- A true spectator that joins without a rider would need a protocol change (admission without spawning, capacity rules); decide whether it is wanted.
