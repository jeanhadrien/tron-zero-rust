# Game Mechanics Design Document

This document outlines the core game mechanics. These should be respected at all times.

## 1. Core Entity: The Player (Lightcycle)

Each player controls a continuous-moving lightcycle.

- **Base Speed:** Players move forward at a constant base speed.

## 2. Movement & Turning

- **Continuous Movement:** Players cannot stop moving unless they hit an obstacle. The physical position is updated each tick based on the current direction, computed speed, and eventual obstacles.
- **Turning:**
  - Players can only turn in fixed degree increments (left/right)
  - Only one turn is executed per tick update.
  - When a player turns, a new turn coordinate is recorded and added to their trail.

## 3. Trails & Obstacles

- **Trail Generation:** As players move, they leave behind turn points. The trail consists of the lines between all the points and the current state position (active trail).
- **Fixed Maximum Trail Length:** Each alive player's total trail arc length (static segments plus the active segment to current position) is capped. Before the cap is reached, the trail grows naturally with movement. Once at cap, each tick shortens the trail from the **tail** (oldest end) by the excess length after movement, so the net arc length stays constant while the head continues to extend.
- **Tail Consumption:** The oldest trail point slides along its segment toward the next point (or current position) before being removed. On a straight path with only one turn point, the tail re-anchors at `TRAIL_MAX_LENGTH` behind the player along the active trail segment (`P₀ → Position`, coincident with heading on axis-aligned straight movement) — a moving player always retains a full-length collidable wall behind them, never a zero-length trail.
- **Collision Lines:** The collidable environment consists of:
  - The outer boundaries of the game area.
  - All player trails including player's own trail.

## 4. Speed Mechanics: Acceleration & Deceleration

The game encourages risky play by rewarding players who ride close to existing trails.

- **Sliding (Acceleration):** If a player is sliding/grinding against an obstacle (trails, walls...), the player is considered "sliding". While sliding, the player accelerates.
- **Deceleration:** If a player is in open space (not sliding), their speed decelerates back down to the baseline over time.
- **Inertia:** The player's actual speed multiplier smoothly interpolates towards the target speed multiplier, meaning acceleration and deceleration have a slight ramp-up/ramp-down.

## 5. Collision & The "Rubber" System

Directly hitting a wall or trail does not instantly kill the player. Instead, the game uses a "Rubber" system.

- **Getting Stuck:** When a player approaches and faces a wall/trail, there's a distance threshold where the speed suddenly drops agressively : the player is visually stopped. Under the hood, the player keeps moving forwarda at a very slow pace. Related mathematical concept: Zeno's paradox (dichotomy paradox). A geometric series where each step is a fraction of the remaining distance, so the limit tends to the wall asymptotically but never reaches it.
- **Speed Drop:** The player's speed drops aggressively in proportion to how close they are to the wall, practically halting them before they cross the line.
- **Rubber Consumption:** While stuck, the player's "Rubber" meter rapidly depletes. The closer the player is pushed against the wall, the faster rubber is consumed.
- **Death:** If the Rubber meter reaches zero, the player dies and the lightcycle is disabled.
- **Rubber Regeneration:** If the player manages to turn away from the wall before dying, the Rubber meter slowly regenerates back to its maximum over time.
