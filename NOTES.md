# Notes

Open feature ideas, not committed work. Neither exists in the code today.

## Audio

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
