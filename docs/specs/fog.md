# Fog of war

Status: proposal 2026-11, not implemented. A room option, off by default. v1
is a client-side experiment to find out whether fog is fun: the server only
says whether fog is on, and each client hides what its rider cannot see.

## Rules

You see the union of these areas:

| Source | Area | Blocked by trails |
|--------|------|-------------------|
| **Cone** | `FOG_CONE_ANGLE` (120°) wide, `FOG_CONE_RANGE` (700 units) deep, centered on the rider's heading. | Yes: every trail, your own included. |
| **Proximity** | A circle of `FOG_PROXIMITY_RADIUS` (90) around your rider. | No. |
| **Glow** | `FOG_GLOW_RADIUS` (72, = `GRIND_RANGE`) around every trail part you can see (cone or proximity), and around **all of your own trail**, seen or not. | No: the glow reaches behind the trail. |
| **Sparks** | A circle of `FOG_SPARKS_RADIUS` (150) around a grinding rider, visible to everyone. It stays `FOG_SPARKS_SECS` (2 s) after the grind ends and shrinks to nothing. | No. |

With the glow radius equal to `GRIND_RANGE`, anyone close enough to grind a
trail you can see is inside its glow.

- **Always visible:** the arena walls and the floor.
- **Hidden in fog:** other riders, their trails, and their effects
  (sparks, derez). A hidden effect would give away the rider.
- **Memory (ghosts):** a trail segment you have seen stays drawn, dimmed, where
  you saw it. Ghosts never fade. A ghost shows the segment, never the rider.
  It disappears when the server clears the trail (death), so a death in the
  fog still shows up as trails vanishing. Accepted for v1: the kill feed and
  the scoreboard announce deaths anyway.
- **Your whole trail glows**, however long. Laying trail lights the map: an
  intended play, to review after testing.
- **The dead stay in fog.** A dead or waiting player keeps the vision of the
  place of death: cone and proximity frozen at the last pose, plus the
  ghosts. Their trail is cleared, so its glow is gone. A player who has not
  ridden yet sees only the ghosts and the arena.
- **Grinding** means `IsSliding` (any obstacle within `GRIND_RANGE`, arena
  walls and your own trail included). It is already replicated for the spark
  particles, so Sparks needs no new data. A rider on the rim is therefore
  always visible: accepted for v1.

Numbers in parentheses are starting values, to tune by playing. The cone
range should stay below what the camera shows, or the fog edge is never on
screen. At the 3.0x cap, 700 units is about 0.65 s of travel.

## Config

A new optional `[fog]` table in the server config (`crates/server/config/`).
Without it, there is no fog.

| Key | Meaning |
|-----|---------|
| `fog.enabled` | Turn fog on. |
| `fog.glow` | Optional, `true` by default. The Glow rule. |
| `fog.sparks` | Optional, `true` by default. The Sparks rule. |

The cone and the proximity circle are not optional. The server replicates
these settings on the match status entity. The client reads them and never
decides on its own. The radii and angles are named consts, not config keys.

## Design

- **Visibility is computed from what is drawn:** the local rider's smoothed
  head and opponents' interpolated trails (`Rendered`). The fog then matches
  the picture, not the simulation.
- **The visibility query lives in `shared/`** as a pure, deterministic
  function: viewer pose and trail segments in, visible region and seen segment
  ids out. Only the client calls it in v1. Bots and server-side fog can call
  it later without moving code.
- **Rendering:** fog is an opaque overlay with holes cut where you can see.
  Riders, trails and effects are drawn below the overlay, so the fog hides
  them for free. Ghosts and arena walls are drawn above it.
- **Tutorial:** no fog.
- **Debug:** a client key shows the arena without fog, for development.

## Cost

The cone is the only expensive part: rays against trails. Glow, proximity
and sparks are circles and capsules with no rays.

- Run it once per rendered frame, for the local rider only.
- Broad phase: keep only the segments whose bounding box meets the cone's
  bounding box. A 700-unit cone covers a small part of the 2000x2000 arena.
- Visibility polygon: one ray toward each kept segment end (plus and minus a
  tiny angle), plus fixed rays along the cone's edges and its arc. The cost
  is rays x segments. Each hit names its segment, which feeds the ghosts.
  `shortcut: O(rays x segments), switch to a sorted angular sweep if
  profiling shows it.`
- Ghosts: per trail, the seen state of each segment. A laid segment never
  moves, so only the head segment changes.

## Out of scope (v1)

- Anti-cheat. A patched client sees everything. Real fog needs server-side
  interest management (Lightyear visibility). Its catch: a client that does
  not know about a hidden trail predicts its rider through it, then rolls
  back to a death.
- Bots. They still see the whole arena.
- Sparks visible only to the owner of the ground trail (`fog.sparks_reveal`).
- A death flash that reveals the place of a death.
- Engine sounds heard through fog (`docs/specs/audio.md`).
- Own-trail glow limited to the last N seconds of trail.
- Sparks only for trail grinds, not arena walls.
