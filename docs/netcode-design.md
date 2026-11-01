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
Admission is a hello handshake on `SessionChannel`: the client's first message
is `SessionHello { protocol, name }`; the server (`admission.rs`) answers
`HelloReply::Accepted { name }` (sanitized, de-duplicated) and only then spawns
the rider, or `Rejected(VersionMismatch | ServerFull)` and closes the link.
Links that never say hello are closed after the session timeout. The hello
messages are registered first in `register_protocol` so a mismatched build can
still decode the rejection. `DisplayName` stays `replicate_once` because the
name is final when the rider spawns.
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

Safe spawning checks a current-geometry corridor, not future moving opponents.

### One-shot turn delivery (implemented and exercised by `crates/nettest`)

Every physical press is one `TurnCommand { seq, direction, generation, tick }`
on the ordered-reliable `TurnChannel`. Lightyear's native `PlayerInput` path
still runs (it feeds client prediction, rollback replay and the input buffer),
but the server ignores it for humans.

- **Client** (`read_keyboard`, FixedPreUpdate): pops at most one queued press
  per fixed tick into the predicted `ActionState` and, in the same system, sends
  the command. `tick` is `LocalTimeline.tick() + input_delay`, i.e. the tick the
  prediction itself applies the turn at (the formula Lightyear's input
  buffering uses). `seq` is assigned at that moment and is monotonic for the
  connection; menu/death queue clears never rewind it. Rollback never runs the
  writer: replays take inputs from Lightyear's recorded history, send nothing
  and consume no keyboard events.
- **Timeline.** The client's `LocalTimeline` is driven by `InputTimeline`,
  which targets `server + RTT/2 + 4*jitter + 1 tick (client margin) + 1 tick
  (server input pipeline) + sync error margin`. It is the same tick count the
  server uses: the native input buffer on the server holds the matching
  `TurnXFor(generation)` at exactly the tick the command claims (checked on every
  command of the clean-link scenario; on impaired links the harness checks that
  each command's tick equals the tick the client predicted the turn at).
- **Server** (`turns.rs`): `collect_turn_commands` (PreUpdate) admits commands
  into a per-link `TurnQueue`; `apply_turn_commands` (FixedUpdate, before
  `simulate_players`) writes every human rider's `ActionState` each tick:
  `None`, or the one turn due. Execution tick = `max(intended, previous turn's
  tick + 1)`, so turns run in order, at most one per rider per tick, and a
  turn that arrives early waits for its tick.
- **Late turns.** A turn due at tick `d` that the server only sees afterwards
  runs on the first tick it can if `now - d <= TURN_LATE_WINDOW_TICKS` (12 =
  100 ms, shifting the corner by at most 36 units, and costing the client one
  rollback); beyond that it is rejected (`TooLate`) with an ack. The server
  never rewrites a simulated tick.
- **Validation.** Commands are rejected at receipt for a stale life
  (`StaleLife`), a dead rider (`NotAlive`), a `seq` not above the highest
  accepted for the current life (`Duplicate`), an intended tick more than
  `TURN_MAX_LEAD_TICKS` (64, Lightyear's own native-input bound) ahead
  (`TooFarAhead`), or a full queue of `TURN_QUEUE_CAP` (32, `QueueFull`).
  Execution never precedes receipt, so backdating a tick gains nothing; at
  worst it makes the turn look late. Queued turns are rejected again at
  execution if the rider died or respawned in between. Queue state lives on the
  link entity and dies with the connection.
- **Double turn.** The overwrite of `ActionState` is unconditional for every
  `Human` rider, every tick. A client that writes native turns without
  commands is delivered into the server's input buffer (the harness sees it
  there) and never turns the rider.
- **Acks.** `Applied { server_tick }` or `Rejected { reason }` per command;
  prediction never waits for them. They are diagnostics today. The client does
  not yet use `Applied.server_tick` to repair its own history of a late turn:
  a late turn costs one ordinary state rollback.

Measured arrival margin (server tick at receipt minus the client's intended
tick; a command is on time while this is <= -1, because the receipt tick is the
last one already simulated; e.g. -4 = 4 ticks of slack),
60 presses per profile, real UDP loopback, Lightyear's receive-side conditioner
on both ends (RTT split evenly; jitter is uniform +/- per direction):

| Profile (RTT / jitter / loss) | min | p50 | p95 | max | Late (>= 0) | Rollbacks |
|-------------------------------|-----|-----|-----|-----|-------------|-----------|
| 0 ms / 0 / 0%                 | -4  | -4  | -4  | -3  | 0 of 60     | 0         |
| 0 ms / +-5 / 0%               | -4  | -4  | -3  | -3  | 0 of 60     | 0         |
| 50 ms / +-10 / 0%             | -7  | -6  | -5  | -4  | 0 of 60     | 0         |
| 150 ms / +-20 / 0%            | -9  | -6  | -4  | -4  | 0 of 60     | 0         |
| 250 ms / +-50 / 0%            | -12 | -4  | 0   | 0   | 4 of 60 (1 tick) | 5    |
| 100 ms / +-30 / 5%            | -9  | -6  | -2  | +11 | 2 of 60     | 4         |
| 150 ms / +-20 / 3%            | -10 | -7  | +7  | +23 | 4 of 50, 2 beyond the window (rejected) | 6 |

One run per row, unseeded conditioner: the lossy rows vary run to run (another
run of the 3% row gave max +19 with one rejection). The 3% row logged only 50 of 60
presses because a rejected turn sent the rider into a wall, and a dead rider
sends no commands.

The premise that no-input-delay makes every turn late was wrong: it makes the
client lead the server by roughly one-way delay plus 4-9 ticks of margin, and
clean links deliver 100% of turns early. Previously the stamp was taken at
press time in PreUpdate (about one tick earlier than the pop tick, and wrong
for every press after the first in a same-frame burst).

Verified by the harness (`cargo test -p tron-zero-nettest`, about 40 s): exact
execution tick and zero rollbacks on a clean link, with client-predicted and
server poses identical at every tick; exactly-once, in-order, never-early
execution and explicit fates under 3% loss and jitter; the late window
(execution ~6 ticks late, and a `TooLate` rejection ~26 ticks late); native-
input cheating; menu swallowing without sequence gaps; forced client rollback
replay; death/respawn and reconnect without turn leakage.

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

The authority model is agreed and one-shot delivery is implemented (see "One-shot
turn delivery" above): turns execute at their intended tick when they arrive
in time and later only within a bounded window, never rewriting a simulated
tick. Client-predicted death can be corrected without reversing a server death.
See `NETWORK.md` for what remains open.

## 2. Simulation Fundamentals

Client and server share a fixed-tick simulation. Cross-platform determinism and
impaired-network behavior still require verification.

- **Fixed Timestep:** Both client and server simulate at 120 Hz (approximately 8.33 ms per tick).
- **Accumulator Pattern:** The game loop translates variable render frames into fixed simulation ticks using an accumulator with rollover and remainder
- **ECS Integration:** Gameplay runs in Bevy `FixedUpdate`, not variable-rate `Update`. Fixed stepping alone does not guarantee identical outcomes when available inputs or collision geometry differ.

## 3. Time Synchronization & Client Lead

During synchronized play, the client's prediction timeline runs **ahead** of
the server so inputs arrive before their intended simulation tick.

Lightyear drives the client's `LocalTimeline` from `InputTimeline` (the
`DRIVING` synced timeline): it targets `server + RTT/2 + 4*jitter + 1 tick +
1 tick + error margin`, and the server's tick numbering is the same counter, so
a client tick is a server tick. The client uses `InputDelayConfig::no_input_delay()`:
no extra local delay, all latency covered by prediction (up to 100 ticks).
Measured result (table above): commands land 4-9 ticks (33-75 ms) before their
tick on links up to 150 ms RTT; at 250 ms RTT with +/-50 ms jitter a few percent
miss by 1-2 ticks. Server input-lead and jitter margins are Lightyear's
defaults; they have not been tuned.

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

Turns travel as sequenced commands on an ordered-reliable channel (Lightyear
retransmits lost packets and delivers in order); the native redundant-input
messages carry prediction only. Verified under 3% loss with jitter: every press
executes once or is explicitly rejected. Outage tolerance beyond the late
window is not promised.

### A. Command identity and retransmission

Commands are identified by (life generation, `seq`); the server executes a
`seq` at most once. Head-of-line blocking is the cost of ordered-reliable
delivery: a lost packet delays every later turn by about one resend round, which
is consistent with the +13..+19 tick arrivals seen in the 3%-loss profile. A
redundant-resend scheme (repeat the last few unacked commands in each message)
would mostly remove that; it has not been built because the lost-turn rate in
the measurements is a few percent at 3-5% loss.

### B. Missing input and timing

Missing input is no turn: the server overwrites Lightyear's repeat-last-input
fallback for humans every tick. Client replay reuses recorded inputs and does
not consume or send anything. The late policy reports `TooLate` rejections and
never replays stale turns after a stall (they expire after the 12-tick window).
Still unmeasured: input starvation under client frame hitches, and long stalls.

## 6. Collision Authority

The server decides collisions from its simulation, not a client's claimed kill,
cutoff or escape. There is no agreed "favor the attacker" rule and no historical
hitbox rewind for trails. Simultaneous-growth resolution and stale opponent
geometry in client prediction remain separate fairness work in `NETWORK.md`.
