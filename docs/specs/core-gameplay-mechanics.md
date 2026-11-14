# Game Mechanics Design Document

Status: verified against `constants.rs` and `physics.rs` on 2026-11-08.

This document outlines the core game mechanics. These should be respected at all times.

## 1. Core Entity: The Player (Lightcycle)

Each player controls a continuous-moving lightcycle.

- **Base Speed:** Players move forward at 360 world units per second at a speed multiplier of 1. The simulation runs at 120 ticks per second.
- **Look:** rider shape and color are chosen by the player and purely cosmetic; collisions use the head point and trail only. A respawn keeps the look.
- **Arena:** a 2000 x 2000 square (5.6 s edge to edge at base speed). Small enough that 2-8 riders meet within seconds; the square shape is assumed by the bots' grids.

## 2. Movement & Turning

- **Continuous Movement:** Players cannot stop moving unless they hit an obstacle. The physical position is updated each tick based on the current direction, computed speed, and eventual obstacles.
- **Turning:**
  - Players can only turn in fixed degree increments (left/right)
  - Only one turn is executed per tick update.
  - When a player turns, a new turn coordinate is recorded and added to their trail.

## 3. Trails & Obstacles

- **Trail Generation:** As players move, they leave behind turn points. The trail consists of the lines between all the points and the current state position (active trail).
- **Fixed Maximum Trail Length:** Each alive player's total trail arc length (static segments plus the active segment to current position) is capped at `TRAIL_MAX_LENGTH` (2400 world units, 6.7 s of base-speed riding, 1.2x the arena side). Before the cap is reached, the trail grows naturally with movement. Once at cap, each tick shortens the trail from the **tail** (oldest end) by the excess length after movement, so the net arc length stays constant while the head continues to extend.
- **Tail Consumption:** The oldest trail point slides along its segment toward the next point (or current position) before being removed. On a straight path with only one turn point, the tail re-anchors at `TRAIL_MAX_LENGTH` behind the player along the active trail segment (`P₀ → Position`, coincident with heading on axis-aligned straight movement) — a moving player always retains a full-length collidable wall behind them, never a zero-length trail.
- **Collision Lines:** The collidable environment consists of:
  - The outer boundaries of the game area.
  - All player trails including player's own trail.
- **Own Trail Junction:** The rider's attachment to its current trail is not itself a collision. Turning away from the most recent corner must remain possible; older sections of the rider's trail remain obstacles.

## 4. Speed Mechanics: Acceleration & Deceleration

The game encourages risky play by rewarding players who ride close to existing trails. All numbers live in `constants.rs`.

- **Sliding (Acceleration):** Modelled on Armagetron's wall acceleration. Each side ray finds the nearest obstacle (trail or wall) within `GRIND_RANGE` (72 units, 0.2 s of base travel) and turns its distance `d` into a strength `(1/(d + GRIND_OFFSET) - 1/(GRIND_RANGE + GRIND_OFFSET)) * GRIND_NORM`: 1 touching, 1/3 at 24 units, 0 at the range. The curve is convex, so touching pays far more than hovering. The two sides are **summed** (grind `0..=2`): riding a tunnel between two trails doubles the acceleration. Arena boundary walls pay `ARENA_WALL_GRIND` (0.4) of the trail strength at the same distance. A rider's **own trail grinds like anyone's**, except the legs nearer than `OWN_GRIND_MIN_FROM_HEAD` (3) segments to the head: a double-tap U-turn leaves its outbound leg one step away, which would otherwise pay ~0.85 for free. Spirals and wider serpentines therefore still pay; a tight serpentine rides on its older legs (an accepted farm until a minimum turn interval exists).
- **Inertia:** `TargetSpeedMult` is the rider's inertial cruise speed `c`. Each tick `c += GRIND_ACCEL * grind + (1 - c) * rate`, then `c = min(c, MAX_SPEED_MULT)`, where `rate` is `SPEED_DECAY` above base speed (the boost fades exponentially, `SPEED_DECAY_PER_SEC` 0.8/s) and `SPEED_RECOVER` below it (5/s). The terminal speed is `1 + GRIND_ACCEL_PER_SEC / SPEED_DECAY_PER_SEC * grind` (0.8/0.8): one trail at contact 2.0x, a tight tunnel approaches the 3.0x cap, the rim alone 1.4x. `SpeedMult` is the realized speed of the tick (cruise, reduced by the rubber zone near an obstacle).
- **Turn penalty:** every executed turn multiplies the cruise speed by `TURN_SPEED_FACTOR` (0.95), floored at `MIN_SPEED_MULT` (0.8) so turn spam cannot stall a rider; below base it recovers at `SPEED_RECOVER`. The penalty applies to the tick's own step.
- **No crash bleed:** facing an obstacle does not change the cruise speed beyond the normal fade; only the step shrinks (section 5).
- **Determinism:** the speed update and the rubber approach use only `+ - * /` and comparisons (no `powi`/`powf`/`exp`; `RUBBER_APPROACH` is a literal), so client and server stay bit-identical across platforms.

## 5. Collision & The "Rubber" System

Directly hitting a wall or trail does not instantly kill the player. Instead, the game uses a "Rubber" system.

- **Getting Stuck:** The rubber zone in front of a rider is as wide as the step it would cancel (`step / RUBBER_APPROACH`), at least `SLOW_DOWN_DISTANCE` (12 units): 12 at base speed, ~32 at the 3.0x cap. Inside it the rider closes `RUBBER_APPROACH` (0.28) of the remaining gap per tick (Armagetron's exponential approach, `1 - e^(-40 dt)`), never more than its normal step (no lurch at the zone edge) and never through the obstacle (`front - EPSILON`). The player is visually stopped but keeps creeping toward the wall.
- **Rubber Consumption:** The distance a tick could not travel (`normal step - actual step`) costs rubber: `lost * RUBBER_COST * 2 / (1 + cruise)`. The zone's outer edge is therefore free and the drain ramps up to full as the rider reaches the wall. Faster riders pay less per unit but lose more units, so they still die sooner: about 22 ticks after contact at base speed (0.18 s), ~14 at the 3.0x cap, which deliberately stays above the 12-tick netcode late-turn window so a correctly timed but late turn can still save a rider.
- **Death:** If the Rubber meter reaches zero, the player dies and the lightcycle is disabled before movement in that tick. Its trail is cleared rather than remaining as a permanent obstacle.
- **Rubber Regeneration:** Every tick, also while grinding or facing a wall, the meter regains `(rubber + RUBBER_REGEN_FLOOR) * RUBBER_REGEN` (`RUBBER_REGEN_PER_SEC` 0.3/s, floor 8), applied before the tick's drain and capped at the maximum. Regen is proportional to what is *left*: a rider who spent 20% (96 left) is back in ~0.7 s, one who spent 50% in ~2.1 s, one who spent 80% (24 left) needs ~4.6 s, and an empty one ~9 s. Burned riders stay fragile; light contact is nearly free. The floor only keeps a near-empty rider recovering at all.

### Rust collision timing

The simulation takes an obstacle snapshot before advancing riders each tick. A
trail cleared on death stops blocking on the following tick. Unlike the original
JS next-update death guard, rubber depletion disables the rider immediately.
Movement is also clamped short of sensed obstacles to prevent high-speed
tunneling through existing geometry. Trail grown during the tick is invisible to
that snapshot, so a second pass sweeps every rider's movement this tick against
the others': at a crossing, the rider who gets there first (by fraction of the
tick) keeps going and the later one stops just short of the new trail; equal
arrival and head-on meetings stop both short of the meeting point, where either
can still turn away. Being stopped costs the rubber of the distance lost, like a tick
facing an obstacle, so two riders tied at a crossing play chicken rather than
wait forever. A rider stopped before a crossing lays no trail there.
Contact tolerances are world-space (0.001 units), so a long straight never
reaches past its corner.

## 6. Human Death & Manual Respawn (Rust MVP)

Death leaves the same owned rider entity disabled with zero speed, velocity,
target speed and rubber, no sliding/collision/death-cleanup flags, and an empty
trail. A red crossed-out rider and **YOU DIED** screen prompt make this visible.
**Space** requests manual respawn (Enter opens the chat box); there is no
client-authoritative or automatic human respawn. Whether a request is accepted
now depends on the server's match rules (`match-rules.md`): during a round with
`respawn = "anytime"` and while waiting for players, yes; with
`respawn = "next_round"` the rider waits and is placed at the next round start.
Bots never ride while no human is connected.

The server accepts a request only for the sending connection's single dead human
and matching life generation. It resets position, cardinal heading, base speed,
velocity, target speed, full rubber, alive/death flags, sliding/collision flags,
trail and current input together, incrementing that generation. Ownership,
identity, color and replication/prediction targets are preserved. Old-generation
turn inputs cannot affect the new life; the physical turn queue is cleared on
death, life/owner changes and disconnect, and dead keypresses are ignored.
The life generation includes a random per-connection nonce, preventing normal
delayed prior-session messages from matching a newly connected rider.

Human spawn selection uses actual arena dimensions and walls, alive trail
segments and rider positions. A candidate needs a 100-unit boundary margin,
80 units of clearance, and a 360-unit forward corridor (one base-speed second).
Seeded sampling followed by a clearance-spaced grid is a bounded search, not an
exhaustive geometric solver. If it finds no safe candidate, the rider remains
dead and the client can retry; no unsafe fallback is chosen. Initial human
spawn uses the same allocator. This is snapshot safety, not spawn invulnerability
or a guarantee against future opponent motion. Bots use the
same allocator: a dead bot is placed again as the same entity after a four-second
delay; bots are never replaced.
