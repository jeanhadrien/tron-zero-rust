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
the rider and starts replicating to the link, or `Rejected(VersionMismatch |
ServerFull)`. A raw UDP "connection" exists as soon as any packet arrives from
an address, so until the hello is accepted the link has no `ReplicationSender`,
at most `MAX_UNADMITTED_LINKS` (32) links may wait (extra ones are closed on
arrival), and links that never say hello are closed after the session timeout.
A rejected link is not closed in the frame the rejection is sent: it lingers
until the client's `Leave` arrives or 1 s passes, so the reliable channel can
retransmit a lost rejection (a rejected client otherwise ended on a
misleading timeout). What the hello compares is `PROTOCOL_ID`: the hand-bumped
`PROTOCOL_VERSION` mixed with a compile-time fingerprint of every constant in
`constants.rs` and the turn timing constants, so a physics retune changes the
id by itself (a test fails if a constant is missing from `SIM_CONSTANTS`; a
change to gameplay *logic* with unchanged constants still needs a manual bump).
The hello messages are registered first in `register_protocol` so a mismatched
build can still decode the rejection. `DisplayName` stays `replicate_once`
because the name is final when the rider spawns.
Names are stripped of control and invisible format characters (bidi overrides,
zero-width), exotic whitespace is normalized, and de-duplication compares a
case- and space-folded `name_key`.
Each link also has a `LinkBudget` (`budget.rs`) counting messages that a
well-behaved client sends rarely: turn batches, rejected turns, session and
respawn requests, in one-second windows. A link over its allowance is closed.
This is basic lifecycle handling and abuse bounding, not authenticated
admission or a replacement for the hardening tracked in `NETWORK.md`.

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
  is cleared when the observed rider entity or its life generation changes, on
  the menu, and on disconnect. It is *not* cleared by the rider being dead:
  `IsAlive` is predicted, so the local rider can be dead only because the
  prediction disagrees with interpolated opponent trails while the server still
  has it alive, and dropping presses then would turn that misprediction into a
  real crash. Presses keep flowing and the server answers `NotAlive` if the
  rider really is dead (a respawn changes the generation, which drops anything
  queued for the old life). Fresh fixed-tick writing and live-queue lifecycle
  processing never run against rollback state; keyboard collection can continue
  using the last fresh observed state. Respawn is not locally predicted: the
  overlay waits for authoritative replication. Predicted death may be corrected
  or precede authoritative death; an early request can be denied and requires a
  new keypress. Confirmation, denial and no-safe-space states have visible prompts.

Safe spawning checks a current-geometry corridor, not future moving opponents.

### One-shot turn delivery (implemented and exercised by `crates/nettest`)

Every physical press is one `TurnCommand { seq, direction, generation, tick }`.
Lightyear's native `PlayerInput` path still runs (it feeds client prediction,
rollback replay and the input buffer), but the server ignores it for humans.
Commands travel as repeated, unreliable `TurnBatch` messages (below), not on a
reliable channel.

- **Client** (`read_keyboard`, FixedPreUpdate): pops at most one queued press
  per fixed tick into the predicted `ActionState` and, in the same system, puts
  the command in the `TurnOutbox`. `tick` is `LocalTimeline.tick() + input_delay`,
  i.e. the tick the prediction itself applies the turn at (the formula
  Lightyear's input buffering uses). `seq` is assigned at that moment and is
  monotonic for the connection; menu/death queue clears never rewind it.
  Rollback never runs the writer: replays take inputs from Lightyear's recorded
  history, send nothing and consume no keyboard events.
- **Redundant delivery** (`input/outbox.rs`): `send_turn_batch` (PostUpdate,
  before `MessageSystems::Send`) sends, at most once per client tick and only
  while the outbox is non-empty, a `TurnBatch` holding *every* command the
  server has not yet reported decided (oldest first, at most
  `TURN_BATCH_MAX` = 16) on the unreliable `TurnChannel`. A lost packet is
  therefore repaired by the next tick's packet (8.3 ms later) instead of a
  reliable resend a round trip and a half later, and a late command is never
  stuck behind an earlier lost one. A command is dropped from the outbox when
  acknowledged, 48 ticks (400 ms) after its stamp (the server would reject it
  `TooLate` anyway), or when 16 newer ones push it out. Cost: a 30-byte
  command is carried by about RTT x 120 batches, i.e. a few hundred bytes/s
  per recent press.
- **Timeline.** The client's `LocalTimeline` is driven by `InputTimeline`,
  which targets `server + RTT/2 + 4*jitter + 1 tick (client margin) + 1 tick
  (server input pipeline) + sync error margin`. It is the same tick count the
  server uses: the native input buffer on the server holds the matching
  `TurnXFor(generation)` at exactly the tick the command claims (checked on every
  command of the clean-link scenario; on impaired links the harness checks that
  each command's tick equals the tick the client predicted the turn at).
- **Server** (`turns.rs`): `collect_turn_commands` (PreUpdate) decides the
  commands of each batch in ascending seq and queues the accepted ones in the
  per-link `TurnQueue`; `apply_turn_commands` (FixedUpdate, before
  `simulate_players`) writes every human rider's `ActionState` each tick:
  `None`, or the one turn due. Execution tick = `max(intended, previous turn's
  tick + 1)`, so turns run in order, at most one per rider per tick, and a
  turn that arrives early waits for its tick.
- **Exactly once, gaps, reordering.** `next_seq` is the dedup watermark: every
  seq below it is *decided* (queued or rejected, rejections included, so the
  client stops resending them). A resent copy of a decided command is dropped
  without a log entry, a reply or any budget cost. A batch always contains all
  the commands its sender still considers undecided, and the client prunes only
  a prefix (on acknowledgement, age or overflow), so a batch holding seq `n+1`
  holds `n` too unless the client deliberately gave `n` up (it was far too old
  to run). The server therefore does not wait for gaps: it skips them. A later
  seq arriving first cannot make an earlier one a duplicate, because they ride
  in the same batch and the batch is processed in ascending order; an old batch
  arriving late only holds decided seqs. (A seq that was given up and arrives
  later anyway, in an older packet, is a copy of a decided seq as well.)
- **Late turns.** A turn due at tick `d` that the server only sees afterwards
  runs on the first tick it can if `now - d <= TURN_LATE_WINDOW_TICKS` (12 =
  100 ms, shifting the corner by at most 36 units, and costing the client one
  rollback); beyond that it is rejected (`TooLate`). The server never rewrites
  a simulated tick. Tick distances use Lightyear's wrapping `Tick` subtraction.
- **Validation.** Commands are rejected at receipt for a stale life
  (`StaleLife`), a dead rider (`NotAlive`), a missing rider (`NotEligible`), an
  intended tick more than `TURN_MAX_LEAD_TICKS` (64, Lightyear's own
  native-input bound) ahead (`TooFarAhead`), or a full queue of
  `TURN_QUEUE_CAP` (32, `QueueFull`). Execution never precedes receipt, so
  backdating a tick gains nothing; at worst it makes the turn look late. Queued
  turns are rejected again at execution if the rider died or respawned in
  between. Queue state lives on the link entity and dies with the connection.
- **Double turn.** The overwrite of `ActionState` is unconditional for every
  `Human` rider, every tick. A client that writes native turns without
  commands is delivered into the server's input buffer (the harness sees it
  there) and never turns the rider.
- **Acks** are diagnostics plus the "stop resending" signal, and are bounded
  by construction. `send_turn_acks` (PostUpdate) sends at most one `TurnAck
  { received_below, newest }` per link per frame on an unreliable, sequenced
  channel, whenever a batch arrived (even an all-duplicate one, so a lost ack is
  repaired by the next batch) or a verdict changed. `received_below` is
  cumulative; `newest` is only the latest execution/rejection since the last
  ack (older verdicts are skipped). Nothing is buffered per command, so a
  client that never acknowledges anything cannot make the server queue replies.
  Prediction never waits for acks. The client does not yet use
  `Applied.server_tick` to repair its own history of a late turn: a late turn
  costs one ordinary state rollback.
- **Abuse bounds** (`budget.rs`): a link sending more than 600 turn batches per
  second (5x the legitimate 120), more than 60 turn commands rejected on
  receipt per second, or more than 10 session or respawn requests per second
  is closed. Oversized batches (> 16 commands) close the link at once.
  Session heartbeat replies, which ride a reliable channel, are limited to one
  per 0.5 s per link, and the respawn channel is budgeted like the session one.

Measured arrival margin (server tick at receipt minus the client's intended
tick; a command is on time while this is <= -1, because the receipt tick is the
last one already simulated; e.g. -4 = 4 ticks of slack), with redundant
delivery. 40 presses per profile (`measure.rs`, staircase of 16-tick steps),
real UDP loopback, Lightyear's receive-side conditioner on both ends (RTT split
evenly; jitter is uniform +/- per direction). The margin is that of the *first
copy* to arrive, which is why the jitter tail shrank: every tick sends a fresh
copy through an independent draw of the jitter:

| Profile (RTT / jitter / loss) | min | p50 | p95 | max | Late (>= 0) | Rejected | Rollbacks |
|-------------------------------|-----|-----|-----|-----|-------------|----------|-----------|
| 0 ms / 0 / 0%                 | -4  | -4  | -3  | -3  | 0 of 40     | 0        | 0         |
| 0 ms / +-5 / 0%               | -4  | -4  | -3  | -3  | 0 of 40     | 0        | 0         |
| 50 ms / +-10 / 0%             | -7  | -6  | -5  | -4  | 0 of 40     | 0        | 0         |
| 150 ms / +-20 / 0%            | -9  | -7  | -5  | -4  | 0 of 40     | 0        | 0         |
| 150 ms / +-20 / 3%, run 1     | -9  | -7  | -5  | -4  | 0 of 40     | 0        | 0         |
| 150 ms / +-20 / 3%, run 2     | -9  | -7  | -5  | -3  | 0 of 40     | 0        | 0         |
| 150 ms / +-20 / 3%, run 3     | -9  | -7  | -5  | -4  | 0 of 40     | 0        | 0         |
| 100 ms / +-30 / 5%            | -9  | -7  | -5  | -4  | 0 of 40     | 0        | 0         |
| 250 ms / +-50 / 0%            | -11 | -8  | -5  | -3  | 0 of 40     | 0        | 0         |

Every command also executed exactly on its intended tick (lateness 0) in every
row. For comparison, the same harness with the previous ordered-reliable
channel (60 presses per profile) gave: 150 ms / +-20 / 3%: max +23 ticks, 4
late, 2 beyond the window and rejected (a second run: max +19, 1 rejected);
100 ms / +-30 / 5%: max +11, 2 late; 250 ms / +-50 / 0%: max 0, 4 late, 5
rollbacks. The headline: a lost packet used to delay every later turn by about
one resend round (13-23 ticks at 150 ms RTT); it now costs nothing visible.

One run per row (three for the 3% profile), unseeded conditioner, so lossy and
jittery rows vary from run to run; the conditioner drops packets independently,
so a long burst of consecutive loss is not covered (nor an outage longer than
the 400 ms the client keeps repeating a command for). The CI test
`redundant_turns_keep_every_press_on_time_at_150ms_and_3_percent_loss` asserts
no rejection and at most 2 late turns of 40 at that profile. The earlier 60-press
version of this table was recorded before the staircase scenario was shortened:
after the arena retune, 60 steps leave the arena and end the run.

The premise that no-input-delay makes every turn late was wrong: it makes the
client lead the server by roughly one-way delay plus 4-9 ticks of margin, and
clean links deliver 100% of turns early. Previously the stamp was taken at
press time in PreUpdate (about one tick earlier than the pop tick, and wrong
for every press after the first in a same-frame burst).

Verified by the harness (`cargo test -p tron-zero-nettest`, about 80 s): exact
execution tick and zero rollbacks on a clean link, with client-predicted and
server poses identical at every tick; exactly-once, in-order, never-early
execution and explicit fates under 3% loss and jitter; no rejection at 150 ms /
3% loss; the late window (execution ~6 ticks late, and a `TooLate` rejection
~26 ticks late); native-input cheating; menu swallowing without sequence gaps;
forced client rollback replay; death/respawn and reconnect without turn
leakage; a flooding client (rejected commands, or duplicate batches) being
disconnected while another client plays on; twenty copies of one command being
one decision; no replication and a bounded link count for spoofed peers;
rejection delivery with 8 clients at 30% loss each way.

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

Turns travel as sequenced commands repeated in every per-tick unreliable batch
until the server reports them decided; the native redundant-input messages
carry prediction only. Verified under 3% and 5% loss with jitter: every press
executes once, on its intended tick, with no rejection (see the table above).
Outage tolerance beyond 400 ms is not promised.

### A. Command identity and retransmission

Commands are identified by (life generation, `seq`); the server decides a
`seq` once. The previous design used an ordered-reliable channel, where a lost
packet delayed every later turn by about one resend round (+13..+19 ticks at
3% loss and 150 ms RTT, one turn beyond the late window). Delivery is now
redundant instead of retransmitted: each tick's batch carries every undecided
command, so a loss is covered by the next packet, the earliest copy wins, and
duplicates cost the server one integer comparison each. Acknowledgments are
cumulative and unreliable (`TurnAck.received_below`), so losing one costs
nothing; the next batch re-triggers it. Bounds: 16 commands per batch and per
outbox, 48 ticks of repetition per command, 600 batches/s per link before the
link is closed.

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
