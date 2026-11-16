# Fog of war

Status: proposal 2026-11, not implemented. A room option, off by default. v1
is a client-side experiment to find out whether fog is fun: the server only
says whether fog is on, and each client hides what its rider cannot see.

## Rules

You see the union of these areas:

| Source | Area | Blocked by trails |
|--------|------|-------------------|
| **Cone** | `fog.cone_angle` (120°) wide, `fog.cone_range` (380 units) deep, centered on the rider's heading. | Yes: every trail, your own included. |
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
screen. The camera shows 800 units of height (`CAMERA_VIEW_HEIGHT`), so 400
above and below the rider, hence the default of 380. The camera does not zoom
out with fog. At the 3.0x cap, 380 units is about 0.35 s of travel.

## Config

A new optional `[fog]` table in the server config (`crates/server/config/`).
Without it, there is no fog.

| Key | Meaning |
|-----|---------|
| `fog.enabled` | Turn fog on. |
| `fog.glow` | Optional, `true` by default. The Glow rule. |
| `fog.sparks` | Optional, `true` by default. The Sparks rule. |
| `fog.cone_angle` | Optional, `120` by default. Full width of the cone, in degrees. |
| `fog.cone_range` | Optional, `380` by default. Depth of the cone, in world units. |

The cone and the proximity circle are not optional. The server replicates
these settings as a `FogRules` component on the match status entity (absent
when fog is off). The client reads them and never decides on its own. The
other radii are named consts, not config keys. Fog applies in every match
phase. `local.toml` (`just play`) turns fog on, for testing.

## Design

- **Visibility is computed from what is drawn:** the local rider's smoothed
  head and opponents' interpolated trails (`Rendered`). The fog then matches
  the picture, not the simulation.
- **The visibility query lives in `shared/`** as a pure, deterministic
  function: viewer pose and trail segments in, visible region and seen segment
  ids out. Only the client calls it in v1. Bots and server-side fog can call
  it later without moving code.
- **Rendering:** fog is an opaque overlay with holes cut where you can see.
  Riders, trails, name labels and effects are drawn below the overlay, so the
  fog hides them for free. Ghosts and arena walls are drawn above it. The
  floor grid under the fog is drawn dimmed by the fog shader itself (from
  world coordinates), so the grid is never drawn over trails.
- **Tutorial:** no fog.
- **Debug:** a client key shows the arena without fog, for development.

## Implementation

Research notes (2026-11). No new dependency is needed.

**Libraries considered, none fits:**

| Option | Why not |
|--------|---------|
| `bevy_firefly` (2D lights, polyline occluders, Bevy 0.19) | WebGPU only. The browser client renders with WebGL2 (`docs/specs/web.md`). |
| `bevy_fog_of_war` (foxzool) | Compute shaders (not in WebGL2), tile chunks, no segment occluders. |
| `geo-visibility` (angular sweep) | `f64` and `geo` types, pulls `geo-clipper` (C++ bindings), a poor fit for wasm and `shared/`. |
| Grid shadowcasting crates | For tile maps; we have segments. |

**Visibility polygon (CPU, `shared/`, about 100 lines, glam `f32`):**

- Every trail and wall is axis-aligned, so a ray against a segment is one
  division and a range check.
- Input size is small: at most `TRAIL_MAX_LENGTH` (2400) units of trail per
  rider, so a few hundred segments for ten riders, plus four walls.
- Broad phase: keep the segments whose bounding box meets the cone's.
- Rays: toward each kept segment end, plus and minus a tiny angle (the
  classic method: [Red Blob Games, 2D visibility](https://www.redblobgames.com/articles/visibility/),
  [ncase, Sight & Light](https://ncase.me/sight-and-light/)), plus fixed rays
  every ~2° for the cone's arc and edges. A hit past the range is cut to the
  range. Sort the hits by angle.
- Each polygon vertex keeps the id of the segment it hit. Two neighbouring
  vertices on the same segment are a visible part of it. That gives the
  Glow area and the ghosts, with no extra query.
- Estimate: ~600 rays x ~300 segments = ~200k cheap tests, below 1 ms native
  in release. `shortcut: O(rays x segments), switch to a sorted angular
  sweep (Red Blob's article, or geo-visibility's algorithm) if
  \`just profile client\` shows it.`

**Mask (GPU, WebGL2-safe, Bevy built-ins only):**

- The visibility polygon is star-shaped around the viewer, so a triangle fan
  from the head is a valid mesh. No triangulation library is needed.
- Glow areas are rectangles: an axis-aligned segment grown by the radius in
  both axes (square ends instead of round ones). Proximity and Sparks are
  `Circle` meshes.
- A mask camera renders these shapes white on black into a texture
  (Bevy `render_to_texture` example), on its own `RenderLayers`, with the
  main camera's transform and projection. Half resolution with linear
  filtering gives soft edges for free.
- A second camera (`order = 1`, no clear) draws a full-screen fog quad with a
  `Material2d` (Bevy `shader_material_2d` example) that darkens where the
  mask is black. Ghosts and arena walls are drawn by that camera too: gizmos
  pick a camera through a `GizmoConfigGroup` with `render_layers`.

**Ghosts:**

- Store the seen interval (one `lo..hi` hull per segment, a shortcut) per
  laid segment. A laid segment never moves; only the head leg grows.
- `Trail::prune` drops corners at the tail, so corner indices shift. Key
  segments by their corner coordinates, or count pruned corners. A pruned
  segment loses its ghost.

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
