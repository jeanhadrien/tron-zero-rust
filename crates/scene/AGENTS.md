# `scene/` — scenes set up on the arena

Root rules: `../../AGENTS.md`. Intent and slices: `plans/tutorial.md` (local, not committed).

## Purpose

A scene is what is set up before riders ride: extra walls, and per rider a
start (fixed pose or safe spawn), speed, rubber, a trail already laid, and a
script of turns or a bot build. Read from TOML, checked, then handed to a
runner as resolved types (`Rider::start` gives the `shared::spawn::Start`).
The lab wraps a scene in its scenario files; the server will apply one for the
tutorial, later perhaps for community servers.

## Boundaries

- Depends on `shared/` and `bots/` only. No runner types, no ECS world access:
  placing riders is the runner's job (`shared::spawn::placed_at`).
- No tutorial vocabulary here (goals, steps, prompts belong to the step file).
- `Scene::from_table` checks what every runner needs; `Scene::check_limits`
  is for hosted scenes (caps, bounds, trails the game could have laid, no
  respawn). The lab does not call `check_limits`: its files use custom arenas.
- The file format becomes a public contract once community servers load it:
  keep keys stable, add rather than rename.

## Known facts

- Inner walls stop riders from both sides in both windings
  (`crates/lab/scenarios/inner_wall.toml`): riders stop 0.002 short, outside
  the exact-contact escape of `contacts`.
- Scene walls are owner-less segments: they grind at `ARENA_WALL_GRIND` (0.4).

## Test

`cargo nextest run -p tron-zero-scene` (parsing and limits). Behaviour of a
scene is checked in the lab.
