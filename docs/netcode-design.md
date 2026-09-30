# Network & Simulation Architecture (Netcode)

## Current Rust MVP implementation

The sections below describe broader design goals, not a complete inventory of
implemented features. The Rust MVP uses Lightyear 0.28 at 120 Hz with the following
simulation contract:

- `SimulationRole::Server` advances all players; the client role advances only
  `Predicted` players. Owning human clients predict their rider; other humans and
  bots use interpolation.
- `Trail(Vec<Vec2>)` is player-owned state, ordered from oldest endpoint through
  corners to the current head. Consecutive pairs include the actively growing
  segment. Spawn initializes two coincident endpoints, tail trimming advances the
  oldest endpoint, and death clears the vector. Trail geometry is replicated and
  predicted as a single component, so turns and trimming roll back together
  instead of spawning untracked child entities.
- Both sides run `simulate_players` in `FixedUpdate`. Server bot input runs before
  it and bot lifecycle runs afterward. Client input writing precedes simulation.
  Each tick freezes obstacle geometry before advancing any rider, avoiding
  dependence on player iteration order.
- Client obstacles from nonpredicted opponents use confirmed trail and alive
  histories sampled at or before the preceding simulation tick. Opponents with
  unavailable history are omitted until history is available. Locally predicted
  obstacles use their live start-of-tick state.
- Position, direction, alive state and trails use step interpolation to keep
  corners and trail heads coherent. Smooth visual interpolation is not yet
  implemented.
- Human respawn uses a dedicated ordered-reliable `RespawnChannel`:
  `RespawnRequest { generation }` is sent client → server and a
  `RespawnReply { generation, outcome }` is returned to that connection.
  Requests carry no player ID, position or client-selected tick. Per-link
  receivers are drained after `MessageSystems::Receive` in `PreUpdate` into a
  server queue because Lightyear clears receivers in `Last`, even on frames
  without a fixed tick. The queue is processed before shared simulation in
  `FixedUpdate`, rechecking connected session ownership, dead human eligibility
  and generation. Accepted/no-space/ineligible outcomes are observable through
  replies and structured server logs. Disconnect invalidates queued requests.
- An accepted respawn atomically resets the existing entity, not its control or
  replication targets. Serialized spawn allocation sees earlier allocations
  in the same tick. `LifeGeneration` is replicated **and predicted**, so
  reconciliation/rollback restores it with movement state. Human turn variants
  carry that generation: a 128-bit value combining a server-generated random
  64-bit connection nonce with a 64-bit life counter. Thus old-session requests
  and turns do not normally match a reconnect, even when both riders are on the
  same life number. Counter exhaustion is denied rather than overflowing the
  nonce. This is a lifecycle discriminator, not transport authentication.
  Old buffered/in-flight turns and untagged human turns are ignored. Bots keep
  untagged turns. Input histories are not erased: rollback can still replay
  the corresponding earlier life.
- Physical turns are accepted only for a ready local rider, and the live queue
  is cleared when the observed rider dies, changes life/identity, or disconnects.
  Dead-state keys are ignored. Fresh fixed-tick writing and live-queue lifecycle
  processing never run against rollback state; keyboard collection can continue
  using the last fresh observed state. Respawn is not locally predicted: the
  overlay waits for authoritative replication. Predicted death may be corrected
  or precede authoritative death; an early request can be denied and requires a
  new keypress. Confirmation, denial and no-safe-space states have visible prompts.

Respawn life tags prevent turns crossing lives, but do not address Lightyear's
same-life one-shot-turn fallback/repetition risk under missing input. That
future-hardening work remains in `NETWORK.md`; it is not implemented here.
Safe spawning checks a current-geometry corridor, not future moving opponents.

Collision is against zero-thickness, start-of-tick segments. Swept front clearance
prevents crossing that geometry, but simultaneous newly grown trail intersections
and head-to-head movement are not swept against each other. Multiplayer rollback
and contact behavior still require runtime playtesting; compilation alone does
not establish network correctness.

## 1. Core Philosophy

The objective is to create a highly responsive network action game. To achieve this, **the client must never wait for the server to validate an action before displaying a response.**

- **Client Prediction:** The player hits a button, the player sees an immediate response.
- **Server Authority:** The client has zero simulation authority other than providing their inputs. The server remains the absolute source of truth to prevent cheating and resolve conflicts.
- **Mispredictions:** Disagreements between the client's predicted state and the server's authoritative state are handled gracefully via deterministic rollback and reconciliation, never at the expense of immediate responsiveness.

## 2. Simulation Fundamentals

Our deterministic simulation relies on a synchronized clock, fixed update intervals, and quantization.

- **Fixed Timestep (Command Frames):** Both client and server operate on quantized "Command Frames" (e.g., 16ms per frame for a 60Hz tick rate).
- **Accumulator Pattern:** The game loop translates variable render frames into fixed simulation ticks using an accumulator with rollover and remainder
- **ECS Integration:** Systems predicting on the client or simulating on the server do not use a variable `update()`. They use an `update_fixed()` step guaranteeing identical integration steps across both ends.

## 3. Time Synchronization & Client Lead

To minimize input delay on the server, the client's simulation clock always runs **ahead** of the server's clock.

- **Lead Formula:** `Client Time = Server Time + (RTT / 2) + 1 Buffered Command Frame`
- **Why?** The client gobbles up input as close to "now" as possible. By simulating ahead of the server by exactly the one-way trip time plus a tiny buffer, the client's input packets arrive at the server at the exact moment the server is ready to simulate that specific command frame.

## 4. Rollback and Reconciliation (Handling Mispredictions)

Because the client simulates ahead, it will occasionally mispredict (e.g., the client thought they turned in time to avoid a trail, but the server calculates they hit it).

- **Ring Buffers:** The client maintains two ring buffers:
  1. **Movement/State Buffer:** The history of the player's simulated states (positions, velocities, active abilities).
  2. **Input Buffer:** The history of the exact inputs (button presses, turns) submitted for each frame.
- **The Reconciliation Loop:**
  1. The client receives an authoritative snapshot from the server for a past tick (e.g., tick 17).
  2. The client checks if its local predicted state for tick 17 matches the server's state.
  3. If they agree, the client ignores the packet and continues.
  4. If they disagree (a misprediction), the client **overwrites** its local tick 17 state with the server's state.
  5. The client then **fast-forwards (replays)** all inputs from tick 18 up to the current predicted tick (e.g., tick 27) to catch back up to "now".

## 5. Network Resilience (Packet Loss & Jitter)

The game uses UDP, which is inherently lossy. We employ two major techniques to ensure simulation stability without sacrificing responsiveness:

### A. Sliding Window Inputs

- Instead of sending just the input for the current frame, the client sends a **sliding window of all inputs** starting from the last frame acknowledged by the server.
- _Example:_ If the server last acknowledged Frame 4, and the client just simulated Frame 19, the packet contains inputs for Frames 5 through 19.
- Since button states compress incredibly well (e.g., "Left Turn was held for 10 frames"), this payload is tiny but guarantees the server can fill in any dropped packets instantly once a subsequent packet arrives.

### B. Dynamic Time Dilation (Buffer Management)

- **Server Starvation:** If the server doesn't receive input in time for a frame, it duplicates the previous input, simulates it, and alerts the client.
- **Client Response (Dilation):** When the client hears the server is starved, it slightly speeds up its local simulation (e.g., ticking every 15.2ms instead of 16ms). This generates inputs slightly faster, inflating the server's input buffer to weather the packet loss/jitter.
- **Client Response (Contraction):** Once the server is healthy and has too large of a buffer, the client slows down its simulation clock (e.g., 16.8ms) to shrink the server's buffer back to the razor's edge, minimizing latency.

## 6. Resolution of Conflicts (Favor the Shooter vs. Mitigating Actions)

- **General Rule:** We favor the attacker/actor. If it looked like a valid kill/cutoff on the attacker's screen, the server will usually validate it.
- **The Exception (Evasive Abilities):** If the victim activated an evasive maneuver (e.g., a hypothetical "Shield" or "Jump" in Tron-Zero) on their client _before_ the attacker's input arrived at the server, the server honors the defensive ability, and the attacker misses.
