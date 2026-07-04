# Tron Zero — Rust Rewrite Plan

## Architecture Overview

```
tron-zero-rs/
  Cargo.toml           (workspace)
  crates/
    shared/             lib   — bevy_ecs components, systems, trail, protocol
    server/             bin   — headless Bevy + lightyear ServerPlugins + bot AI
    client/             bin   — Bevy rendering + bevy_egui + lightyear ClientPlugins
```

The Node.js `server-manager` (lobby/matchmaking) stays in JS — it's a simple CRUD API.

**Both client and server depend on `shared/`**, which contains all ECS components, systems, and the lightyear protocol definition.

**Client compiles to both native (desktop) and WASM (browser)** from the same Rust codebase via Bevy's platform support. Single-threaded on WASM for v1 (no web worker) — Bevy ECS + lightyear should fit the frame budget at 120 Hz × 10 players. If profiling later shows frame drops on WASM, move the simulation to a web worker via `wasm-bindgen-rayon` (requires COOP/COEP headers from the manager); the Bevy schedule itself stays unchanged. Marked as **explore later**.

> **Note on lightyear API names:** specific type/method names below and throughout this plan (`VisualInterpolationPlugin`, `ServerTransports::Udp`, `WebTransportCertificateSettings::AutoSelfSigned`, `Interpolated`/`Predicted` markers, `MessageEvent<T>`, `send_message_to_target`, `InputBuffer`, `TimeManager`, `app.replicate::<T>()`, `tick_duration` on `ClientPlugins`/`ServerPlugins`, the PreUpdate/PostUpdate schedule placement) are **speculative — taken from search snippets, not verified against the lightyear 0.28 source**. The implementer should treat them as hints, read the actual lightyear 0.28 docs.rs / book / crate source, and use the real API names. These are not load-bearing for the design; only the gameplay/simulation spec is.

## Library Choices

| Concern | Library | Why |
|---|---|---|
| ECS | **bevy_ecs** (0.19) | Queries, observers, change detection, native `ChildOf` relation. Best Rust ECS feature match for bitECS. |
| Rendering + windowing | **bevy** (0.19) | Full engine. Compiles to native (wgpu) and WASM (WebGL/WebGPU). Replaces Phaser 3 entirely. |
| Networking | **lightyear** (0.28) | Server-authoritative, tick-based, built-in prediction + rollback + interpolation + predicted entity spawn. Replaces bitecs serializers + geckos.io + ClockSyncManager + StateReconciler + TickPipeline + AuthoritativeDeltaApplier + SnapshotRing + EntityIdMapStore + NetworkProtocol. |
| Visual interpolation | **lightyear `VisualInterpolationPlugin`** | Lerps `Position`/`Direction` in `PostUpdate` via `Time<Fixed>::overstep_percentage()`; restores canonical value in `PreUpdate`. Replaces the JS 500-tick render ring + alpha extrapolation. |
| Transport | **lightyear (UDP)** for dev, **WebTransport** for prod | UDP for simplicity during development. WebTransport (QUIC) for browser + native production — same codebase, feature flag swap. |
| UI | **bevy_egui** (0.31) | Immediate-mode GUI. Main menu, HUD, chat, server browser, settings. Replaces SolidJS. |
| Math | **glam** (0.29) | Vec2, fast 2D ops. Comes with bevy_ecs. Replaces custom math.ts. |
| Serialization | **serde** + **postcard** | serde for component serialization, postcard via lightyear's default. |
| Logging | **tracing** | Structured, spans. Replaces OpenTelemetry + custom Logger. |
| Bot AI / spawn RNG | **rand** | PRNG for bot strategy/name selection and bot rotation. **Not** used for spawn placement (that's the deterministic `mulberry32`, ported verbatim — see Player Lifecycle). |
| Audio | **bevy_kira_audio** | Engine sounds, explosion on death, spatial listener. Replaces Phaser `AudioManager`. |
| Manager HTTP | **reqwest** | `POST /api/rooms`, heartbeat, unregister. Replaces the JS fetch/axios calls in `server/main.ts`. |

## Core Design Decision: Predicted State Replication

We chose lightyear's **`Predicted`** model (state replication) over `Deterministic` (inputs-only).

**Rationale:** Tron has cross-entity interactions — player A's trail affects player B's collisions. If B's client state diverges from the server, A's collision checks on that trail are also wrong. The deterministic model requires *every* entity to be byte-identical, or the whole simulation is corrupt. That's fragile across native/WASM float differences.

With `Predicted`:
- Server sends authoritative state for all player entities each tick (Position, Direction, SpeedMult, etc.)
- Client predicts ahead with local inputs
- On mismatch, lightyear rolls back to the earliest known-good tick and replays `FixedMain` forward
- This is identical to the JS codebase's model: apply server diffs → resimulate

Bandwidth cost is negligible — 10 players × a few Vec2/f32 fields per tick.

## Tick Schedule

**FixedUpdate at 120 Hz** (~8.33ms, matches the JS codebase's `GameClock` default `1000/120`). `tick_duration = Duration::from_secs_f64(1.0/120.0)` on both `ClientPlugins` and `ServerPlugins`.

The JS `PlayerSystem.update` is 3 phases. Mirror them as ordered Bevy system sets inside `FixedUpdate::Main`:

```
Set: LifecycleTurns   (Phase 1)
  - Drain GameEvent queue: PlayerJoined → create_player, PlayerSpawn → spawn_player, PlayerLeft → remove_player
  - For each player: if dead or Rubber <= 0 → disable_player; else if input.turn → execute_turn
    (execute_turn: append TrailPoint child at current Position, rotate Direction by ±90°)

Set: GridSync          (Phase 2)
  - No-op for v1 (naive collision — see Spatial Strategy). Reserved for a future grid.

Set: CollisionMovement (Phase 3)
  - For each alive player:
    - Build 3 sensor rays (front, left, right) from Position + Direction
    - Query nearest trail/wall segment along each ray (naive: iterate all segments)
    - Rubber-zone response, rubber drain/regen, slide boost
    - Position += Velocity / 1000
    - enforce_trail_max_length (trim tail to 200 * tick_ms world-units)
```

Server-only (runs after Phase 3):
```
Set: ServerBotDecide
  - Bot AI: ray fan → candidates → strategy weights → cooldown → emit turn inputs for next tick
  - Bot rotation (every 10000ms of sim time): replace a random bot
  - Dead bots: emit PlayerSpawn event

lightyear internal (own schedule slots, not in FixedUpdate::Main):
  - PreUpdate: apply client inputs, apply replicated state
  - PostUpdate: send replication, collect inputs
```

**Do not reorder the phases.** Splitting movement/turn/collision/rubber into separate systems in a different order changes behavior (e.g. rubber regen before vs after movement; turn after movement vs before).

## Components

### Player

| Component | Type | Replicated? | Predicted? | Notes |
|---|---|---|---|---|
| `Player` | marker | yes (for spawning) | — | |
| `PlayerId` | `String` | yes | no | Stable identity across respawns |
| `Position` | `Vec2` | yes | yes | World position |
| `Direction` | `Vec2` | yes | yes | Unit vector heading (one of ±X, ±Y; rotates by ±90° on turn) |
| `Velocity` | `Vec2` | yes | yes | Per-tick displacement ×1000 (Position += Velocity/1000 each tick) |
| `SpeedMult` | `f32` | yes | yes | Current speed multiplier |
| `TargetSpeedMult` | `f32` | yes | yes | Drifts toward 1 when not sliding; boosted when sliding |
| `Rubber` | `f32` | yes | yes | 0 = death. Clamped to `[0, BASE_RUBBER]` |
| `PlayerColor` | `u32` | yes | no | Packed RGB `(r<<16)|(g<<8)|b`, each channel in `[0x66, 0xFF)` |
| `IsAlive` | `bool` | yes | yes | Toggled, **not removed** on death. Entity persists for respawn/reconnect. |
| `ShouldHandleDeath` | `bool` | yes | yes | Guards one-shot death handling next Phase 1 |
| `IsSliding` | `bool` | yes | yes | Side-ray within SLOW_DOWN_DISTANCE |
| `IsColliding` | `bool` | yes | yes | Front-ray within SLOW_DOWN_DISTANCE |
| `TrailPointCount` | `u32` | yes | yes | Mirrors child count for quick checks |
| `TrailPointNextOrder` | `u32` | yes | yes | Monotonic counter; **predicted** so it rolls back with the player |

### Trail (child entities of Player)

| Component | Type | Replicated? | Notes |
|---|---|---|---|
| `TrailPoint` | marker | yes | Tag |
| `TrailPointOrder` | `u32` | yes | Sort key for trail rendering. **Derived deterministically from input+tick** so client and server assign the same value (see Predicted Trail-Point Spawning). |
| `Position` | `Vec2` | yes | World coordinate of the turn point |
| `Direction` | `Vec2` | yes | Post-turn heading at this point |
| `ChildOf` | bevy 0.19 native `ChildOf` | yes | Parent = Player entity. Auto-despawns children when the player entity is removed (not when disabled). |

### Arena

| Component | Type | Replicated? | Notes |
|---|---|---|---|
| `Arena` | marker | yes | Single entity |
| `AreaWidth` | `f32` | yes | 2400 default; spawn bounds + grid dims |
| `AreaHeight` | `f32` | yes | 2400 default |
| `WallSegments` | `Vec<Vec4>` | yes | 4 boundary walls (x1,y1,x2,y2) |

### Bot-only (server, NOT replicated)

| Component | Type |
|---|---|
| `BotBrain` | strategy + decision state, keyed by `PlayerId` (not eid — rotation removes/recreates entities) |
| `BotMemory` | recent decisions, positioning; `reset()` on death |

## Player Lifecycle

Port `PlayerSystem.ts` verbatim in behavior. Four distinct operations:

- **`create_player(player_id)`** — spawn a **dead** entity (`IsAlive=false`, defaults at origin, `TrailPointCount=0`, `Rubber=BASE_RUBBER`, `SpeedMult=0`, `TargetSpeedMult=1`). Called on `PlayerJoined` event. Color via `generate_player_color` (RGB each `[0x66,0xFF)`, packed).
- **`spawn_player(player_id, seed_tick)`** — place an existing dead entity at a **deterministic** position and wake it. Position via `mulberry32(hash_string(player_id) ^ seed_tick)`: `x = 100 + rng*(width-200)`, `y = 100 + rng*(height-200)`, `direction = floor(rng*4)*90°`. Set `IsAlive=true`, `ShouldHandleDeath=true`, create the initial `TrailPoint` child at the spawn location. **Port `hash_string` + `mulberry32` exactly** (`PlayerSystem.ts:384-401`) — both client predict and server must compute the same spawn. Called on `PlayerSpawn` event.
- **`disable_player(eid)`** — on death: zero `SpeedMult`/`TargetSpeedMult`/`Velocity`/`Rubber`, set `IsAlive=false`, `ShouldHandleDeath=false`, `IsSliding=false`, `IsColliding=false`, **remove all child trail points**, reset `TrailPointCount`/`TrailPointNextOrder` to 0. The player entity **lives on as dead** so `PlayerId` stays stable for reconnect/respawn.
- **`remove_player(player_id)`** — full entity removal (on `PlayerLeft`). Children auto-despawn via `ChildOf`.

Constants: `BASE_SPEED=360`, `BASE_RUBBER=120`, `DELTA_STUFF=12`, `ROTATION_ANGLE=π/2` (90°), `SLOW_DOWN_DISTANCE=12`.

## Movement & Collision (Phase 3 detail)

This is the core gameplay, ported from `PlayerSystem.ts:640-728`. For each alive player each tick:

1. **Sensor rays** (`buildDetectionLines`): front (heading), left (heading rotated −90°), right (heading rotated +90°). `look_ahead_length = max(2000, BASE_SPEED * SpeedMult * 0.5)`.
2. **Nearest-segment query** along each ray. v1: naive — iterate every trail segment (all players' trail-point pairs + last trail point → current Position, i.e. **active segments**) and arena walls; return min positive-t intersection. Include the player's own active segment only via the `includeActiveFor` rule (own trail is an obstacle except the segment immediately ahead of the rider — match JS).
3. **Rubber zone** (`distFront < SLOW_DOWN_DISTANCE`):
   - `IsColliding = true`
   - `rubber_speed_ratio = distFront² / SLOW_DOWN_DISTANCE²`
   - `step = distFront * rubber_speed_ratio` (Zeno deceleration); set `Velocity` so `Position += step` along heading.
   - `Rubber -= DELTA_STUFF * 0.03 * (1 + TargetSpeedMult)³` (drains faster at higher speed).
4. **Not in rubber zone**:
   - Regenerate: `Rubber += 0.006 * tick_ms * DELTA_STUFF` (clamped to `BASE_RUBBER`).
   - Restore normal speed: `Velocity = heading * (BASE_SPEED * TargetSpeedMult * tick_ms / 1000)`.
5. **Slide boost**: if `distLeft < SLOW_DOWN_DISTANCE || distRight < SLOW_DOWN_DISTANCE` → `TargetSpeedMult *= 1.003^(DELTA_STUFF/16.666)`, `IsSliding = true`. Else if not colliding and `TargetSpeedMult > 1` → decay toward 1 by `0.0003 * DELTA_STUFF`.
6. **Move**: `Position += Velocity / 1000`.
7. **Trail cap**: `enforce_trail_max_length` — if `arc_length > 200 * tick_ms`, consume from the tail (`consume_trail_from_tail` / `consume_trail_from_tail_pure` from `trail.ts`). Arc length = sum of segment lengths + active segment.
8. **Clamp** `Rubber` to `[0, BASE_RUBBER]`.
9. **Death**: when `Rubber <= 0`, set `ShouldHandleDeath=true`; `disable_player` runs in the next tick's Phase 1.

`Direction` is a `Vec2` unit vector (±X/±Y). Turn = rotate by ±90° (swap components + sign). Velocity is axis-aligned: `(BASE_SPEED * SpeedMult * tick_ms / 1000, 0)` or `(0, …)` depending on heading. Sensor rays use the Vec2 heading + its 90° rotations.

## Spatial Strategy (v1)

**No spatial grid for v1.** Naive collision: each player's 3 ray queries iterate all trail segments (every player's trail-point pairs + active segments + arena walls). At 10 players × ~200 trail points × 3 rays × 120 Hz this is cheap enough; revisit only if profiling warrants it.

**Dropped from the JS port (not needed for v1):** `SpatialGrid`, `SpatialGridSystem`, `gridTraversal` (DDA), `segmentRaster`, `trailDiff`, `SpatialGridMutator`, `activeSegments` as a separate module (active segments are computed inline during the ray query from `Position` + last `TrailPoint`).

**Bot AI still needs a neighborhood cell structure** for `CorridorFreedom.measureFreedom` (BFS reachable area). Scope it locally: derive blocked cells on the fly from trail segments within the BFS max-radius (12 cells, `BotAiBudget.BFS_MAX_RADIUS`). A lightweight local grid scoped to the BFS neighborhood is acceptable; do not port the global `SpatialGrid`.

## Bot AI (server-only)

Port the full JS bot stack. Modules:

| Path | Ports from | Role |
|---|---|---|
| `server/bot/brain.rs` | `BotBrain.ts` | Layered labyrinth-aware decision engine: ray fan → candidates → strategy weights → cooldown |
| `server/bot/strategy_weights.rs` | `BotStrategyWeights.ts` | 4 strategies: `CutOff`, `BoxIn`, `SpeedDemon`, `Trapper` — per-candidate score modifiers |
| `server/bot/scorer.rs` | `TurnCandidateScorer.ts` | Evaluate possible turns (hold/left/right) with projected freedom + trap score |
| `server/bot/threat.rs` | `EnemyThreatModel.ts` | `project_position`, `compute_trap_score` |
| `server/bot/memory.rs` | `BotMemory.ts` | Recent decisions; `reset()` on death |
| `server/bot/degradation.rs` | `BotDegradation.ts` | 3 tiers: `Full`/`Tier1`/`Tier2`, selected from `ticks_in_batch` (>1→T1, >3→T2) and per-frame AI budget; skips BFS/lookahead at higher tiers |
| `shared/spatial/corridor.rs` | `CorridorFreedom.ts` | BFS reachable area + cardinal exits (local neighborhood grid) |
| `shared/spatial/raycast.rs` | `BotRaycastSensing.ts` | `cast_ray_fan` sensor intersection |

Constants (`BotAiBudget`): `BFS_VISIT_BUDGET_CURRENT=800`, `BFS_VISIT_BUDGET_LOOKAHEAD=400`, `BFS_MAX_RADIUS=18`, `LOOKAHEAD_TICKS=4`, `REAR_RAY_LENGTH=400`, `PER_TICK_BUDGET_MS=2.0`, `PER_FRAME_BUDGET_MS=5.0`, `ACTION_COOLDOWN_TICKS=3`, `TRAPPER_COOLDOWN_TICKS=12`, `SURVIVAL_THRESHOLD_BASE=28`, `SURVIVAL_THRESHOLD_SLIDE=12`, `SURVIVAL_THRESHOLD_MAX=50`, `ENTRAPMENT_ESCAPE_THRESHOLD=78`, `FRONT_PRESSURE_DISTANCE=120`, `HUNT_RANGE=900`, `TRAP_SCORE_MULTIPLIER=1.4`.

Behavior:
- **Auto-respawn**: dead bots emit `PlayerSpawn` next tick.
- **Rotation**: every `BOT_ROTATION_INTERVAL_MS=10000` of sim time (derive ticks from `tick_ms`), replace a random bot — emit `PlayerLeft` + `PlayerJoined` + `PlayerSpawn`. Port this (it's a test-only feature but exercises the join/leave flow).
- **Cooldowns**: `should_skip_input` — skip if pre-queued input exists, or `tick - last_action_tick < ACTION_COOLDOWN_TICKS`, or `tick < cooldown_until_tick` (TRAPPER extended).
- `BotBrain` keyed by `PlayerId` (not eid) because rotation removes/recreates entities.

## Game Events

`shared/` defines a `GameEvent` enum + a local bevy `Event`/resource queue, drained at the top of Phase 1 each tick:

```rust
pub enum GameEventType {
    PlayerJoined, PlayerLeft, PlayerSpawn, PlayerDeath, PlayerTurn,
    GameStart, GameStop, GamePause,
}
pub struct GameEvent { pub tick: u32, pub event_type: GameEventType, pub player_id: Option<String> }
```

- **PlayerJoined → `create_player`**, **PlayerSpawn → `spawn_player`**, **PlayerLeft → `remove_player`** (in Phase 1).
- **Wire path**: join/leave entity replication is automatic via lightyear's replication API; `RespawnRequest`, `GameStart/Stop/Pause`, and `ChatMessage` are **lightyear Messages** (`MessageEvent<T>`), not bevy Events and not `PlayerInput`. The server translates a `RespawnRequest` message into a local `PlayerSpawn` GameEvent.
- **Chat bridge**: `gameEventToText` (`ServerChatSystem.ts:12-32`) converts `PlayerJoined`/`PlayerLeft`/`GameStart`/`GameStop`/`GamePause` into `ChatMessage { type: Event, … }`. (`PlayerSpawn`/`PlayerDeath`/`PlayerTurn` are intentionally not bridged.)

## Predicted Trail-Point Spawning (highest-risk integration point)

Trail points are entities spawned during simulation (on turn execution). Under lightyear `Predicted`:

- Spawn the `TrailPoint` child inside the predicted system on the client (with `Predicted` + `ChildOf(player)`). lightyear matches it to the server's replicated spawn on confirmation and rolls back the spawn on misprediction. This **replaces** the JS manual `reconcileIdMapWithWorld` dedup pass (`SimulationPipeline.postTick`).
- **`TrailPointOrder` must be derived deterministically from input+tick**, not from a per-entity counter the client guesses independently. Port `TrailPointNextOrder` as a **predicted component** on the player so the counter rolls back with the player; both sides then assign the same order value for the same logical turn.
- Verify against the lightyear `simple_box` predicted-spawn example first. This is the single most likely place for a port bug (duplicate `TrailPointOrder` → undefined sort → diagonal trail segments).

## Systems & Modules (shared/)

| Path | Ports from | Role |
|---|---|---|
| `components/player.rs` | PlayerSystem.ts | All component definitions |
| `components/arena.rs` | GameArenaSystem.ts | Arena + wall components |
| `systems/player.rs` | PlayerSystem.ts | Lifecycle, turn, movement, rubber, collision, trail cap, events |
| `systems/arena.rs` | GameArenaSystem.ts | Arena init (2400×2400, 4 walls) |
| `spatial/corridor.rs` | CorridorFreedom.ts | Local BFS freedom (bot AI) |
| `spatial/raycast.rs` | BotRaycastSensing.ts | Sensor ray fan (bot AI) |
| `trail/trail.rs` | trail.ts | `consume_trail_from_tail`, arc length, `is_trail_at_cap` |
| `util/rng.rs` | PlayerSystem.ts:384-401 | `hash_string` + `mulberry32` (deterministic spawn) |
| `protocol.rs` | NetworkProtocol.ts, PlayerInput.ts, ChatMessage.ts, GameEvent.ts | lightyear `Input`, `Message`, and local `GameEvent` types |

## Per-Crate Implementation

### Phase 1 — `shared/` (simulation core)

All ECS components, systems, and modules. No networking — pure Bevy ECS.

- Components with `Serialize`/`Deserialize` derive for lightyear replication
- Systems are plain functions registered in `FixedUpdate::Main` as ordered sets
- Uses `glam::Vec2` throughout for consistent float behavior
- Naive collision (no grid) for v1

### Phase 2 — `server/` (headless)

Headless Bevy app (`MinimalPlugins`) + lightyear `ServerPlugins`.

```
server/src/
  main.rs         — app setup, server config, transport (UDP/WebTransport)
  bot/
    brain.rs         — BotBrain: strategy decision engine
    strategy_weights.rs — 4 strategy score modifiers
    memory.rs        — BotMemory: recent state
    scorer.rs        — TurnCandidateScorer: evaluate possible turns
    threat.rs        — EnemyThreatModel
    degradation.rs   — BotDegradation: 3-tier budget scaling
  systems/
    bot.rs           — ServerBotSystem: inject bot PlayerInput, rotation, auto-respawn
    chat.rs          — ServerChatSystem: event→chat bridge, broadcast ChatMessage
  manager.rs         — reqwest HTTP: register, heartbeat, unregister with Node manager
```

### Phase 3 — `client/` (rendering + UI)

Full Bevy app (`DefaultPlugins`) + lightyear `ClientPlugins` + `bevy_egui`.

```
client/src/
  main.rs            — app setup, client config
  render/
    player.rs        — trail lines (static + active), rider sprites, name labels
    arena.rs         — grid lines, boundary walls
    camera.rs        — follow local player, zoom, smooth interpolation
  input/
    config.rs        — keyboard → PlayerInput mapping, keybinding definitions (persisted)
  ui/
    menu.rs          — main menu, server browser via HTTP to manager
    hud.rs           — speed gauge, rubber meter, alive/dead
    chat.rs          — chat panel sending/receiving ChatMessage (egui-backed Vec)
    settings.rs      — volume, key rebinding
  audio/
    manager.rs       — bevy_kira_audio, spatial listener, engine/explosion SFX
```

### Phase 4 — Polish

- Audio (engine sounds, explosion on death)
- Spectator mode (follow another player after death)
- Session reconnect handling (lightyear handles this)
- Production WebTransport setup with self-signed cert + digest injection via manager

## Transport Strategy

**Development:** UDP (no certs, zero setup)
```rust
transport: ServerTransports::Udp { local_port: 3000 }
transport: ClientTransports::Udp
```

**Production (WASM + native):** WebTransport (QUIC)
```rust
transport: ServerTransports::WebTransport {
    local_port: 3000,
    certificate: WebTransportCertificateSettings::AutoSelfSigned(/* SANs */),
}
transport: ClientTransports::WebTransport
```

WebTransport works on both native (`wtransport` crate) and WASM (browser's native `WebTransport` API). For WASM, the server's certificate digest must be injected into the client page — the Node manager can serve this at page load time.

## Protocol Definition

```rust
// Inputs (client → server) — lightyear Input type, NOT a Message.
// Rollback/replay/rebroadcast handled by lightyear's InputBuffer.
#[derive(Serialize, Deserialize, Clone, Default)]
pub enum PlayerInput {
    #[default]
    None,
    TurnLeft,
    TurnRight,
}
// Note: no Respawn variant. break/alpha from JS are dead fields — dropped.

// Messages (bidirectional) — lightyear Message, delivered as MessageEvent<T>.
#[derive(Serialize, Deserialize, Clone)]
pub struct RespawnRequest; // client → server; server emits a PlayerSpawn GameEvent

#[derive(Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub tick: u32,
    pub timestamp: u64, // ms epoch
    pub message_type: ChatType, // Player | Event
    pub player_id: Option<String>,
    pub text: String,
    pub color: Option<u32>,
}
#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub enum ChatType { Player, Event }

// GameStart/GameStop/GamePause are separate Message types (server → clients).
```

## Manager HTTP Protocol (`server/manager.rs`)

The Rust server speaks the existing Node manager's REST API via `reqwest`:

- `POST /api/rooms` with `RegisterPayload { host, port, secure, display_name, max_players }` → returns `{ id }`.
- `POST /api/rooms/:id/heartbeat` with `HeartbeatPayload { player_count }` every 2s.
- `DELETE /api/rooms/:id` on SIGINT/SIGTERM.
- `secure` flag mirrors the JS `ADVERTISED_SECURE` (true when client is HTTPS).
- Env vars: `MANAGER_URL`, `SERVER_NAME`, `MAX_PLAYERS`, `PORT`, `ADVERTISED_PORT`, `ADVERTISED_SECURE`.

## What lightyear handles (we don't build)

- Component state replication (lightyear `app.replicate::<T>()`)
- Client-side prediction + rollback (`Predicted` marker)
- Predicted entity spawn matching + rollback (trail points)
- Remote entity interpolation (`Interpolated` marker + `VisualInterpolationPlugin`)
- Input buffering, rollback replay, rebroadcast (`InputBuffer` + generic inputs plugin)
- Time synchronization between client and server (`TimeManager`)
- Entity ID mapping across peers
- Connection lifecycle (connect, disconnect, auth, reconnection)
- Transport I/O (UDP or WebTransport)

## JS modules that vanish entirely (no Rust counterpart)

`ClockSyncManager`, `ClientNetworkSystem`, `SimulationWorkerManager`, `WorkerProtocol`, `simulation.worker`, `SimulationPipeline`, `AuthoritativeDeltaApplier`, `StateReconciler`, `SnapshotRing`, `EntityIdMapStore`, `NetworkProtocol`, `PlayerInputBuffer`, `TickRingBuffer`, `GameEventBuffer` (replaced by lightyear Messages + bevy Events for local scheduling), `ChatMessageBuffer` (replaced by an egui-backed `Vec`).

## What we build

- All game-specific components, systems, and modules
- Player lifecycle (create/spawn/disable/remove) + deterministic spawn RNG
- Movement & collision/rubber/slide model
- Naive segment collision (v1; no spatial grid)
- Bot AI (server-only): brain, strategies, scorer, threat, memory, degradation, rotation, auto-respawn
- Trail management (tail consume, arc length, cap)
- Client rendering (trails, arena, camera)
- Client UI (menus, HUD, chat, settings)
- Client input handling (keyboard → `PlayerInput`)
- Audio
- Manager heartbeat (server → Node lobby)

## Determinism Contract

- All sim systems use `f32` (via glam `Vec2`) — consistent across platforms
- `Direction` constrained to ±X/±Y (no trig at turn sites; 90° rotations are component swaps + sign flips)
- System execution order is explicitly defined (3 ordered sets in `FixedUpdate::Main`)
- Entity iteration order must be deterministic — use sorted queries or ordered collections
- Spawn position deterministic via ported `hash_string` + `mulberry32`
- `TrailPointOrder` derived deterministically from input+tick (predicted counter rolls back)
- No `sin`/`tan` approximations that differ per platform (sensor rays use Vec2 heading + 90° rotations; `look_ahead_length` uses no trig)
