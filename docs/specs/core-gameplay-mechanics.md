# Game Mechanics Design Document

Status: verified against `constants.rs` and `physics.rs` on 2026-11-02.

This document outlines the core game mechanics. These should be respected at all times.

## 1. Core Entity: The Player (Lightcycle)

Each player controls a continuous-moving lightcycle.

- **Base Speed:** Players move forward at 360 world units per second at a speed multiplier of 1. The simulation runs at 120 ticks per second.
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

- **Sliding (Acceleration):** A rider is "sliding" when a side ray finds an obstacle (trail or wall) within `SLIDE_DISTANCE` (24 units, wider than the 12-unit front slowdown zone). Closeness sets a *goal speed*: `1 + (MAX_SPEED_MULT - 1) * (1 - distance / SLIDE_DISTANCE)`, so touching the wall aims for the cap (2.0x, 720 u/s) and the edge of the range gives nothing. Grinding rewards risk near *other* riders, so a rider's **own trail never grinds** (no farming a double-tap U-turn corridor), and arena boundary walls pay only `ARENA_WALL_GRIND` (0.4) of the trail strength at the same distance: edge riding aims for at most 1.4x (504 u/s) even at contact, versus the 2.0x cap on an opponent's trail.
- **Inertia:** `TargetSpeedMult` is the rider's inertial cruise speed. Each tick it moves toward the goal speed at bounded rates and never leaves `[1, MAX_SPEED_MULT]`: up at `SLIDE_ACCEL_PER_SEC` (0.5/s, base to cap in ~2 s of tight grinding), down at `COAST_DECEL_PER_SEC` (0.5/s) in open space or above the goal. Boost must be fed continuously and fades within about 2 s. `SpeedMult` is the realized speed of the tick (cruise, reduced by the Zeno slowdown near a wall).
- **Crash braking:** while the front ray is inside the slowdown zone, cruise speed drops at `CRASH_BRAKE_PER_SEC` (3.0/s) and does not accelerate. Brushing a wall bleeds boost.
- **Determinism:** the speed update and Zeno slowdown use only `+ - * /` and comparisons (no `powi`/`powf`/`exp`), so client and server stay bit-identical across platforms.

## 5. Collision & The "Rubber" System

Directly hitting a wall or trail does not instantly kill the player. Instead, the game uses a "Rubber" system.

- **Getting Stuck:** When a player approaches and faces a wall/trail within 12 world units, speed drops aggressively: the player is visually stopped. Under the hood, the player keeps moving forward at a very slow pace. The Zeno step is `front * (front / 12)^2`, never more than the rider's normal step (otherwise entering the zone would lurch the rider forward). Related mathematical concept: Zeno's paradox (dichotomy paradox). A geometric series where each step is a fraction of the remaining distance, so the limit tends to the wall asymptotically but never reaches it.
- **Speed Drop:** The player's speed drops aggressively in proportion to how close they are to the wall, practically halting them before they cross the line.
- **Rubber Consumption:** While stuck, the player's "Rubber" meter rapidly depletes from a maximum of 120. As in the original JS game, drain increases with the cruise speed multiplier: `RUBBER_DRAIN * (1 + TargetSpeedMult)^3` per tick, `RUBBER_DRAIN = DELTA_STUFF * 0.03 = 0.36`. That is about 0.35 s to die at base speed and about 0.1 s (13 ticks) at the 2.0x cap, which deliberately stays at or above the 12-tick netcode late-turn window so a correctly timed but late turn can still save a rider. Obstacle distance controls slowdown, not this drain rate.
- **Death:** If the Rubber meter reaches zero, the player dies and the lightcycle is disabled before movement in that tick. Its trail is cleared rather than remaining as a permanent obstacle.
- **Rubber Regeneration:** If the player manages to turn away from the wall before dying, the Rubber meter regenerates at `RUBBER_REGEN` = 0.15 per tick (18/s, empty to full in ~6.7 s) whenever the front is clear. Tapping a wall is cheap; camping on the rubber is not (drain outpaces regen ~19:1 at base speed).

### Rust collision timing

The simulation takes an obstacle snapshot before advancing riders each tick. A
trail cleared on death stops blocking on the following tick. Unlike the original
JS next-update death guard, rubber depletion disables the rider immediately.
Movement is also clamped short of sensed obstacles to prevent high-speed
tunneling through existing geometry. Simultaneous newly grown trail intersections
and head-to-head movement are not yet swept against each other.

## 6. Human Death & Manual Respawn (Rust MVP)

Death leaves the same owned rider entity disabled with zero speed, velocity,
target speed and rubber, no sliding/collision/death-cleanup flags, and an empty
trail. A red crossed-out rider and **YOU DIED** screen prompt make this visible.
**Space / Enter** requests manual respawn; there is no client-authoritative or
automatic human respawn.

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
or a guarantee against future opponent motion. Bots retain their existing
random spawns, two-second death respawns and thirty-second replacement cycle.
