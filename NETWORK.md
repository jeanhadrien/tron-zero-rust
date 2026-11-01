# Netcode hardening backlog

Status: updated 2026-11-01 (second session). P0 one-shot turn delivery is
implemented, now with redundant resend and bounded replies, and verified by the
headless network harness (`crates/nettest`, run with
`cargo test -p tron-zero-nettest`; measurements with
`cargo test -p tron-zero-nettest --test measure -- --ignored --nocapture`).
Unchecked items are not implemented or verified. The implemented contract and
measured numbers are in `docs/netcode-design.md` ("One-shot turn delivery").

## Done in the second 2026-11-01 session (review fixes)

- [x] **Turn loss under packet loss.** Turns no longer ride an ordered-reliable
  channel. The client repeats every undecided command in a per-tick unreliable
  `TurnBatch` (at most 16 commands, dropped 48 ticks after their stamp); the
  server decides each seq once and ignores resent copies. Head-of-line blocking
  is gone. Measured at 150 ms RTT, +-20 ms jitter and 3% loss (three
  unseeded 40-press runs): every command arrived at least 3 ticks early and ran
  on its intended tick, none rejected, against arrivals up to +23 ticks late
  and one `TooLate` per run with the ordered-reliable design. 100 ms / 5% and
  250 ms / +-50 ms jitter were also clean (one run each). Redundancy shrinks
  the jitter tail too, since the earliest of many copies wins. Table in
  `docs/netcode-design.md`.
- [x] **Ack amplification.** The per-command reliable `TurnAck` is gone. The
  server sends one coalesced, unreliable ack per link per frame (cumulative
  `received_below` plus the newest verdict); duplicates cost no reply and no log
  entry. A per-link `LinkBudget` (`crates/server/src/budget.rs`) closes links
  that exceed 600 turn batches/s, 60 receipt-time rejections/s, or 10 session
  or respawn requests/s. Heartbeat replies are limited to one per 0.5 s. The
  respawn channel had the same unbounded-reliable-reply shape and got the same
  budget.
- [x] **Replication to unadmitted links.** `ReplicationSender` is inserted when
  the hello is accepted, not on connect; at most 32 links may wait for a hello.
- [x] **Rejection reply sent once.** A refused link stays open until the
  client's `Leave` arrives or 1 s passes, so the reliable channel can
  retransmit the rejection (checked with 8 clients at 30% loss each way).
- [x] **Predicted death swallowing presses.** Presses keep flowing while the
  predicted rider is dead; the queue is cleared only on rider/life change, menu
  and disconnect. The server answers `NotAlive` if the rider is really dead.
- [x] Autoconnect/quit races, `ticks_between` wrap safety, name sanitizing and
  de-dup, restarting the deadline after a momentary not-ready, symmetric
  latency-sim uplink, debounced atomic preference writes, `PROTOCOL_VERSION`
  bump plus a compile-time fingerprint of the sim constants in the hello.

## Done in the 2026-11-01 session

- [x] Traced the Lightyear 0.28 input pipeline: `LocalTimeline` on the client is
  driven by `InputTimeline` (targets `server + RTT/2 + 4*jitter + 2 ticks +
  sync margin`), the native input buffer is stamped with the same tick numbers,
  the server applies native inputs in FixedPreUpdate with a repeat-last-input
  fallback, and native messages beyond 64 ticks ahead are dropped.
- [x] Measured the arrival margin (receipt minus intended tick): commands arrive
  4-9 ticks early on links up to 150 ms RTT, so intended-tick scheduling works.
  The earlier "every turn is late with no input delay" premise was wrong.
- [x] Chose bounded late execution: on time at the intended tick; late within
  `TURN_LATE_WINDOW_TICKS = 12` runs on the first free tick; later is rejected
  (`TooLate`) with an ack. `TURN_MAX_LEAD_TICKS = 64`, `TURN_QUEUE_CAP = 32`.
- [x] Fixed: press-time stamping (wrong for same-frame bursts), the in-life
  `seq` reset on menu/queue clears (made every later press a `Duplicate`), and
  the Lightyear last-input fallback being able to add or invent turns.
- [x] Headless server + client(s) harness over real UDP with the link conditioner.

## Lightyear behaviour worth remembering

- **Timeline resync is a relabel, not a rewind.** `InputTimeline` emits a
  `SyncEvent` on the first sync (a large positive delta, ~+45 ticks) and again
  whenever the error exceeds `max_error_margin` (seen at 150 ms / 3% loss: a
  -11 tick jump shortly after joining, when lost pings skewed the estimate).
  The rider keeps simulating forward from its current state; only the tick
  numbers jump, and Lightyear shifts its prediction/input histories by the same
  delta (`update_ticks`). No turn runs twice: each fixed step still consumes the
  `ActionState` once. A backward jump does make the client's state differ from
  the server's for that tick label, so a normal rollback follows. The nettest
  client trace follows `SyncEvent`s (`follow_timeline_resync`) so relabelled
  ticks are not reported as "ran twice outside rollback". The client turn
  outbox follows the same event (`input::follow_timeline_resync`) and shifts its
  undecided stamps (see `docs/netcode-design.md`, timeline resync).

## Still open after that session

- [ ] Late turns cost a state rollback on the client. The ack carries
  `Applied { server_tick }`; the client could patch its input history with it
  to avoid the visual snap. Not done.
- [ ] Loss beyond redundancy's reach: a command is repeated for at most 48
  ticks (400 ms) after its stamp and the outbox holds 16, so an outage longer
  than that still loses turns (explicitly rejected or never seen, never
  invented). Bursts of consecutive loss (as opposed to the conditioner's
  independent drops) are not measured.
- [ ] 12-tick window is a measured-but-untuned default (one 36-unit corner shift
  at most). Needs real-player data before it is called competitive-ready.
- [ ] Not measured: frame hitches/catch-up bursts on the client, long stalls
  and outages, 2+ simultaneous clients playing together, sequence behaviour
  across a *natural* Lightyear resync under real jitter (a forced one is
  covered by a nettest). The abuse budget limits are reasoned, not tuned
  against real client traces.
- [ ] Spoofed-source floods are only bounded (32 unadmitted links, 5 s hello
  timeout, no replication, one tiny ack per frame): raw UDP has no handshake,
  so a flood of distinct source addresses still costs a link entity each.
- [ ] Unseeded conditioner: runs are not reproducible; assertions are
  invariants, not exact traces. No saved seeds/traces yet.

## Assessment and existing foundation

The current architecture is a reasonable MVP base, not yet a demonstrated
resilient or esports-ready implementation. Keep Bevy + Lightyear rather than
rewriting the networking stack.

- Bevy provides ECS and scheduling; Lightyear 0.28 provides networking,
  synchronization, input transport, prediction, replication, and rollback.
- Client and server share a 120 Hz simulation (8.33 ms per tick).
- The server is authoritative; clients predict their own rider.
- Keyboard events preserve order, including multiple presses in one frame.
  One turn is consumed per fresh tick, with no configured extra input-delay tick.
  Rollback does not consume the live keyboard queue.
- Player-owned trail geometry participates in replication and prediction.
- Opponents use step interpolation; collision prediction uses confirmed history.
- A localhost-only overlay now owns explicit connect/cancel/disconnect/retry,
  with separate connection/menu state, session heartbeats, readiness gating,
  and client/server timeouts. Runtime lifecycle checks remain outstanding.

Local responsiveness is not proof of online correctness. Turn delivery now has a
real-UDP harness (loopback, link conditioner, up to 250 ms RTT); load, long
outages, multi-player collision fairness and the lifecycle overlay under
impairment remain unverified.

## P0: correct one-shot turn delivery

Relevant files: `crates\client\src\input\keyboard.rs`,
`crates\shared\src\components\player.rs`, `crates\shared\src\protocol.rs`,
and `crates\shared\src\systems\player.rs`.

Lightyear's current server input fallback predicts missing input from the last
known state. Our `TurnLeft` / `TurnRight` values mean "turn once," not "continue
holding a direction." Repeating that state can cause an extra authoritative turn.
The rollback-safe local queue does not fix this network-level mismatch.

- [x] Trace the installed Lightyear input pipeline and its extension points
  before choosing a protocol; do not fork or replace the transport by default.
- [x] Define ordered turn commands with identity and intended simulation tick.
  Preserve left/right ordering and distinguish consecutive identical turns.
- [x] Make command-consumption state rollback-aware (client replay reuses the
  recorded input history and never sends; verified with a forced rollback). Replaying a historical tick
  must reproduce its turn, while receiving a duplicate packet must not add one.
- [x] Define missing-input behavior: continue movement, never invent a turn.
- [x] Define bounded redundancy/retransmission, acknowledgment, deduplication,
  history retention, sequence wraparound, and reconnect/session reset semantics.
  Done: per-tick resend of the undecided commands (16 max, 48 ticks), cumulative
  coalesced unreliable acks, dedup by a per-connection `next_seq` watermark
  (every verdict, rejections included, advances it), bounded queue, per-link
  state that dies with the connection, u64 seq. A gap in the seqs is skipped,
  not awaited: batches are supersets of the unacked set, so a gap only means
  the client gave that command up.
- [x] Decide the late-command policy explicitly: reject with feedback or apply
  within a bounded late window. Server rewind is excluded from this design.
  Evaluate fairness and complexity before selecting one. Sequence IDs alone
  cannot restore an action to a tick the server has already simulated.
- [x] Validate ownership, tick windows, command ordering, and the legal turn
  rate server-side (one per rider per tick; future bound 64; no backdating). Bound memory/work without silently dropping accepted input.
- [ ] Define keyboard queue behavior on focus loss, death, disconnect, reconnect,
  and prolonged synchronization stalls; avoid stale turns firing after recovery.
  Verified: menu, predicted death (presses kept, server judges), respawn,
  disconnect/reconnect. Not verified: focus loss and sync stalls. Known cost: a
  really dead player's key presses are still sent and rejected `NotAlive`.

Acceptance: within the supported network envelope, accepted commands execute
once in order in the authoritative timeline. Client rollback reproduces the same
result. Missing input never generates a turn. Late/rejected commands have an
explicit, observable outcome. No promise of delivery through an unlimited outage.

### Agreed authority model (2026-10-01)

Menu/HUD work is checkpointed in `8a83345`. Turn-delivery implementation has not
started. The agreed architecture is:

- **Forward-only authoritative server:** late input does not rewrite simulated
  ticks, relocate historical corners, or reverse authoritative deaths.
- **Immediate local prediction:** fresh queued turns execute at most once per
  client simulation tick, without waiting for acknowledgment.
- **Client reconciliation/rollback:** restore authoritative state and replay
  outstanding historical inputs. Replay must not consume fresh keyboard events
  or manufacture additional commands.
- **Missing input means no new turn:** continue normal movement/collision
  simulation; never repeat a one-shot action just because its packet is missing.
- **No server world rollback or shooter-style historical hit testing** in this
  turn-delivery work. Server authority alone does not prohibit those techniques,
  but neither is required to make this command pipeline reliable.

Client-predicted deaths can still be corrected by server snapshots; that does
not mean the server reversed its own earlier death decision.

### Late-command decision (resolved 2026-11-01: bounded late execution)

The options considered, and what was chosen (12 ticks, see the status above):

- **Strict deadline:** reject commands received after their intended server tick.
  Preserves already-published history but can discard physical presses under jitter.
- **Bounded late execution:** apply late commands in order on subsequent ticks,
  at most one per tick, and explicitly reject expired commands. The 100 ms /
  12-tick window was chosen; it is a tuning default, not a supported-network
  guarantee. It cannot restore the intended corner
  or reverse an authoritative death.

Input timeline lead and command redundancy can reduce missed deadlines without
waiting to display a local predicted turn. They should be measured before
choosing a lateness budget; RTT is not itself command lateness.
Whichever policy is selected, transport receipt, scheduling acceptance and final
simulation outcome must be distinguished in acknowledgments and diagnostics.
Timing validation must be server-bounded, not based on arbitrary client backdating.

### Implementation sequence

1. Trace installed Lightyear 0.28 input collection, tick mapping, buffering,
   redundancy, receipt acknowledgments, missing-input fallback, pruning and
   client replay. Record actual defaults/extension points before changing them.
   Reuse the existing stack where it satisfies the contract.
2. Specify the command contract: session/life identity, distinct ordered turns,
   intended tick, server consumption state, and explicit applied/rejected
   outcomes. Define sequence gaps, duplicate/conflicting submissions, expiry,
   history limits and resets. Confirm the late policy before implementing it.
3. Implement end-to-end delivery and rollback-aware consumption together.
   Preserve same-frame bursts and consecutive identical turns, one per tick.
   Bound network history/work without silently losing already accepted commands.
   Do not assume an ordered-reliable channel alone solves tick deadlines.
4. Add command/tick traces and deterministic regressions for loss, duplication,
   reordering, delayed packets, missing sequence gaps, client rollback, death,
   respawn and reconnect. Explicitly assert that no input invents a turn and
   that each accepted command executes exactly once in authoritative simulation.
5. Exercise the real client/server pipeline under seeded network impairment.
   Measure lateness, input lead, corrections and queue age before tuning buffers.
   Done via `crates/nettest` (unseeded conditioner).

Reference distinction: Riot's
[VALORANT netcode explanation](https://www.riotgames.com/en/news/peeking-valorants-netcode)
separates committed server movement with client correction from historical hit
registration. This is architectural context, not a specification of Lightyear
or a claim that all esports games implement identical networking.

## P1: collision prediction and competitive fairness

Relevant file: `crates\shared\src\systems\player.rs`.

- [ ] Verify that opponent histories exist on the expected entities and are
  sampled consistently during normal prediction and rollback.
- [ ] Define behavior when opponent history is missing or stale. Currently a
  missing history omits that opponent from predicted collision geometry.
- [ ] Measure prediction disagreement caused by older opponent trails, especially
  newly grown heads, recently trimmed tails, rubber contact, and death cleanup.
- [ ] Define and implement simultaneous trail-growth / head-to-head resolution.
  Current sweeps cover start-of-tick segments, not simultaneous new crossings.
- [ ] Specify equal-tick ties, death timing, trail removal, and near-contact
  tolerances. Ensure results do not depend on ECS iteration order.
- [ ] Choose a consistent latency/fairness policy for close cutoffs. Do not
  blindly import shooter-style lag compensation or trust client-reported kills.
- [ ] Verify replay consistency, including floating-point behavior across
  supported platforms; shared code and fixed ticks alone do not prove determinism.

Acceptance: scripted contacts, cutoffs, simultaneous crossings, and rubber
escapes have repeatable authoritative outcomes. Corrections are explainable by
documented rules rather than missing geometry or order-dependent simulation.

## P1: diagnostics and reproducible network tests

Add diagnostics before tuning input lead, packet redundancy, or send frequency.

- [ ] Add opt-in structured tracing correlating session/player, command ID,
  intended tick, send/receive tick, applied/rejected result, and rollback range.
  Partly done: the opt-in server `TurnLog` (queued/applied/rejected with ticks)
  and the harness traces; rollback ranges and the client side are not logged.
- [ ] Record RTT, jitter, loss estimates, input starvation, input lead, queue
  depth/age, correction count/distance, rollback depth/cost, and bytes per second.
  Distinguish measured loss from application-level missing input.
- [ ] Measure key-event-to-predicted-turn and predicted-turn-to-visible-frame
  latency; separately measure authoritative acknowledgment. Do not label RTT
  or tick duration as total input latency.
- [x] Add a headless server + client integration harness over real UDP with
  scripted presses (`crates/nettest`). One client per scenario so far, no seeded
  conditioner (Lightyear's uses unseeded `rand`), so it is not deterministic;
  a two-client scenario is still to be written.
- [ ] Include same-frame bursts, consecutive same-direction turns, mixed turns,
  frame hitches, catch-up ticks, rollback during input, and long-running sessions.
  Done: bursts, mixed turns, forced rollback during input. Not done: hitches,
  catch-up ticks, long sessions.
- [ ] Exercise joining/leaving during play, reconnects, server restart, late
  spawn replication, and missing opponent history. Done: reconnect, respawn.
  Not done: server restart, opponents.
- [ ] Save seeds, impairment profiles, version/configuration, command traces,
  and authoritative outcomes so failures can become regression fixtures.

Initial test matrix (proposed scenarios, not promised supported limits):

| Dimension | Cases |
|-----------|-------|
| RTT | Loopback, 30, 80, 150, 250 ms |
| Added per-direction delay variation | 0, 5, 20, 50 ms |
| Independent packet loss | 0%, 1%, 3%, 5% |
| Burst impairment | 100-500 ms outage; duplication and reordering |
| Asymmetry | Different upstream/downstream delay and loss |
| Rendering | 30, 60, 144+ FPS; temporary client stalls |
| Server load | Intended player cap, dense trails, sustained rapid turns |

Use a small representative CI matrix and a broader scheduled/manual matrix,
not every Cartesian combination. Define jitter distributions and random seeds.
Beyond the supported envelope, require clear degraded/disconnected behavior
instead of pretending controls remain guaranteed.

## P2: presentation, replication, and capacity

- [ ] Separate render smoothing from authoritative collision state. Smooth
  movement/corrections without rounding cardinal corners or detaching heads
  from trails. Current position/direction/trail interpolation is stepped.
- [ ] Profile full `Trail(Vec<Vec2>)` replication, snapshot size, history memory,
  rollback replay, and brute-force collision scanning at intended player counts.
  A trail length cap does not by itself establish a safe vertex-count budget.
- [ ] If measurements justify it, evaluate trail deltas, quantization,
  replication cadence, and spatial collision indexing. Preserve rollback and
  late-join reconstruction; do not optimize away correctness.
- [ ] Measure server tick-time p50/p95/p99 and worst-case stalls against the
  8.33 ms tick budget, with headroom for network processing.
- [ ] Define supported player count, bandwidth/memory budgets, and correction
  frequency/distance targets from representative hardware and network profiles.

## P2: connection and deployment hardening

- [ ] Replace hardcoded loopback address/port with explicit configuration.
- [x] Add localhost connecting/synchronizing/disconnected feedback, explicit
  retry, and documented heartbeat/connection timeouts. Runtime smoke checks
  remain required; degraded-network diagnostics are still pending.
- [ ] Add a server browser fetching a remote master-server list. Reuse the
  connection lifecycle, not a second network client or separate gameplay scene.
- [ ] Review production transport/session authentication and admission control;
  current links use `RawClient` / `RawServer`. Do not treat server authority as
  a substitute for authenticated sessions and bounded input processing.
- [x] Verify incompatible protocol/build rejection (also under 30% loss),
  disconnect cleanup, and stale-session isolation. The id compared is
  `PROTOCOL_ID` (version + sim-constant fingerprint).
- [ ] Document supported deployment topology, UDP/firewall requirements,
  diagnostics collection, and operational limits before public hosting.

## Repository and tooling additions to evaluate

Keep work in this repository. No new repository, dependency, or service is
required merely to start this backlog.

| Addition / candidate | Purpose and constraints |
|----------------------|-------------------------|
| Headless integration test target or small workspace harness crate | DONE: `crates/nettest` (server/client lib split + real UDP sessions, scripted presses, trace assertions). |
| Development-only network impairment configuration | DONE for tests: Lightyear's receive-side `LinkConditioner` on both ends (latency, jitter, loss; jitter reorders). Unseeded, so not reproducible; no duplication or burst outages. |
| PowerShell scenario runner | Start only scenario-owned processes, gather artifacts, and clean up those processes by PID. |
| Existing `tracing` infrastructure | Structured command/tick diagnostics first; avoid introducing a metrics backend until needed. |
| Linux `tc netem` | Optional external UDP impairment on an isolated CI host/network namespace; do not alter shared host networking. |
| Windows clumsy / WinDivert | Optional manual UDP impairment; evaluate maintenance, permissions, and compatibility before installation. |
| Wireshark | Optional packet capture for actual UDP cadence, sizes, fragmentation, and loss investigation; application traces are still needed. |
| `proptest` | Optional command/trail sequence generation and shrinking, if deterministic fixtures leave gaps. |
| CI workflow under `.github\workflows` | Formatting, checking, linting, executed regressions, and bounded network scenarios with failure artifacts. |

Tool names are candidates, not newly verified compatibility recommendations.
Check current upstream documentation and licensing before adopting them.
Use the installed Lightyear version's source and examples rather than assuming
newer documentation describes 0.28 APIs.

The harness runs in plain `cargo test` (about 40 s, serialized because it uses
wall-clock timing).

## Completion gate and documentation

- [ ] Execute existing gameplay/input regressions, then add protocol, collision,
  and end-to-end impairment regressions as fixes land. Done for turns: unit
  tests plus the harness; collision regressions remain.
- [ ] Establish and publish numerical acceptance budgets before calling the
  game competitive-ready; do not infer them from the framework or tick rate.
- [ ] Demonstrate no missing/duplicate accepted turns within the supported
  envelope (demonstrated to 250 ms RTT and 3% loss on loopback; envelope not
  yet declared), repeatable collision outcomes, bounded resource use (done for
  replies and links, see above), and controlled behavior outside that envelope.
- [ ] Update `docs\netcode-design.md` to distinguish implemented behavior from
  aspirational claims about quantization, input acknowledgment windows, time
  dilation, and conflict resolution.
- [ ] Keep `docs\dev.md` aligned with actual scenario commands and diagnostics.

Suggested order: command tracing and turn semantics, protocol regressions,
two-client impairment harness, collision fairness, then presentation/capacity
and production hardening. Do not delay instrumentation until the end.
