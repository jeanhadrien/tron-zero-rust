# Network & Simulation Architecture (Netcode)

## Current Rust MVP implementation

The client now starts disconnected and connects to localhost only on request.
Menu visibility is separate from the connection lifecycle: opening the overlay
captures fresh gameplay input but does not stop simulation or networking.
`crates\client\src\connection.rs` owns connect, readiness, timeout, and teardown;
`menu.rs` owns the overlay. A future master-server browser can feed the same
connection entry point; discovery is not implemented.

Raw UDP's local `Connected` marker does not prove a remote server is present.
An application heartbeat runs every second, with a 5-second liveness timeout
on both peers and a 10-second client connection/synchronization deadline.
Gameplay readiness additionally requires input timeline sync and replicated
arena/local-rider state. A leave message is flushed before client unlink;
server lease expiry handles dropped leave messages and abrupt exits.
Local rider readiness uses the replicated `Controlled` marker, not the
server-side `ControlledBy` relationship. Input-marker setup runs after replication
and tolerates either arrival order of `Player` and `Controlled`.
This is basic lifecycle handling, not authenticated admission or a replacement
for the turn-delivery hardening tracked in `NETWORK.md`.

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

## 1. Agreed Authority Model (2026-10-01)

The objective is to create a highly responsive network action game. To achieve this, **the client must never wait for the server to validate an action before displaying a response.**

- **Client Prediction:** The player hits a button, the player sees an immediate response.
- **Server Authority:** The client has zero simulation authority other than providing their inputs. The server remains the absolute source of truth to prevent cheating and resolve conflicts.
- **Forward-only Server:** Late commands do not rewind the world, rewrite past trails, or reverse authoritative deaths. Full server rollback and shooter-style historical hit testing are outside the turn-delivery design.
- **Mispredictions:** The client restores authoritative state and replays historical inputs. This is client rollback, not server rollback. Corrections can still be visible; immediate prediction does not guarantee agreement.
- **One-shot Turns:** Every physical press is a distinct ordered command, including repeated turns in the same direction. Missing input must never create another turn. Normal movement and collision handling continue when no new turn is available.

The authority model is agreed; reliable one-shot delivery is still pending.
Strict rejection versus bounded later execution of late commands remains an
explicit decision. Neither may change an already-simulated server tick.
Client-predicted death can be corrected without reversing a server death.
See `NETWORK.md` for implementation order, acceptance criteria and open decisions.

## 2. Simulation Fundamentals

Client and server share a fixed-tick simulation. Cross-platform determinism and
impaired-network behavior still require verification.

- **Fixed Timestep:** Both client and server simulate at 120 Hz (approximately 8.33 ms per tick).
- **Accumulator Pattern:** The game loop translates variable render frames into fixed simulation ticks using an accumulator with rollover and remainder
- **ECS Integration:** Gameplay runs in Bevy `FixedUpdate`, not variable-rate `Update`. Fixed stepping alone does not guarantee identical outcomes when available inputs or collision geometry differ.

## 3. Time Synchronization & Client Lead

During synchronized play, the client's prediction timeline runs **ahead** of
the server to give inputs time to arrive before their intended simulation tick.

Lightyear manages synchronization and timeline lead. The client currently uses
`InputDelayConfig::no_input_delay()`: no configured extra delay before applying
local input, not zero network transit time or zero prediction lead.
The installed implementation and actual lead must be traced/measured before
tuning. `RTT / 2 + buffer` is only an intuition, not a verified configuration or
a guarantee of arrival before a deadline; routes can be asymmetric and jittery.

## 4. Client Rollback and Reconciliation (Handling Mispredictions)

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

The game uses UDP with Lightyear's input transport. The following are requirements
for the pending turn-delivery work, not claims of completed hardening.

### A. Redundancy and Command Identity

Inspect and reuse Lightyear's actual redundant-input facilities before adding
another channel. Commands need unambiguous identity/order and bounded history;
packet receipt is not proof that a turn executed. Retransmission must not repeat
an action, and two consecutive left turns must remain two commands.
Redundancy improves recovery but cannot guarantee timely delivery through an outage.

### B. Missing Input and Timing

Repeating held movement can be appropriate in other games; repeating a Tron
turn is not. Lightyear's current last-input fallback is a known mismatch to fix,
not the desired gameplay rule. Missing input must mean no new turn.
Client replay must restore consumption state with gameplay state, while fresh
input collection remains separate from replay.

Measure input starvation, lead, command age and corrections before tuning
buffering. The late-command policy must report expired/rejected commands and
must not replay stale turns indefinitely after a stall.

## 6. Collision Authority

The server decides collisions from its simulation, not a client's claimed kill,
cutoff or escape. There is no agreed "favor the attacker" rule and no historical
hitbox rewind for trails. Simultaneous-growth resolution and stale opponent
geometry in client prediction remain separate fairness work in `NETWORK.md`.
