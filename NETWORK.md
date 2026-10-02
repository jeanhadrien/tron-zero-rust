# Netcode hardening backlog

Status: planning only, 2026-09-30. These items are not implemented or verified
unless explicitly described as existing behavior below.

## Next session TODO (paused 2026-10-01)

- [ ] Start with the P0 Lightyear 0.28 input-pipeline investigation: trace tick
  mapping, input lead, redundancy, acknowledgments, missing-input fallback and
  replay; document actual defaults and available extension points.
- [ ] Use that evidence to choose strict late-turn rejection versus bounded
  later execution with the user. No late policy or 100 ms allowance is approved.
- [ ] Implement the one-shot command contract end to end, preserving rapid
  queued presses, ordering, deduplication and rollback/life/session safety.
- [ ] Add regressions and command traces, then arrange user-run or external
  impaired-network validation before claiming delivery reliability.

Agreed boundary: forward-only authoritative server, immediate client prediction,
client reconciliation only. Missing input never invents a turn. No server rewind.
Work is paused before investigation/implementation; resume here next session.

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

Local responsiveness is not proof of online correctness. Runtime multiplayer,
packet-impairment, and load behavior remain unverified. Existing regression tests
have been compiled but not executed during this work.

## P0: correct one-shot turn delivery

Relevant files: `crates\client\src\input\keyboard.rs`,
`crates\shared\src\components\player.rs`, `crates\shared\src\protocol.rs`,
and `crates\shared\src\systems\player.rs`.

Lightyear's current server input fallback predicts missing input from the last
known state. Our `TurnLeft` / `TurnRight` values mean "turn once," not "continue
holding a direction." Repeating that state can cause an extra authoritative turn.
The rollback-safe local queue does not fix this network-level mismatch.

- [ ] Trace the installed Lightyear input pipeline and its extension points
  before choosing a protocol; do not fork or replace the transport by default.
- [ ] Define ordered turn commands with identity and intended simulation tick.
  Preserve left/right ordering and distinguish consecutive identical turns.
- [ ] Make command-consumption state rollback-aware. Replaying a historical tick
  must reproduce its turn, while receiving a duplicate packet must not add one.
- [ ] Define missing-input behavior: continue movement, never invent a turn.
- [ ] Define bounded redundancy/retransmission, acknowledgment, deduplication,
  history retention, sequence wraparound, and reconnect/session reset semantics.
- [ ] Decide the late-command policy explicitly: reject with feedback or apply
  within a bounded late window. Server rewind is excluded from this design.
  Evaluate fairness and complexity before selecting one. Sequence IDs alone
  cannot restore an action to a tick the server has already simulated.
- [ ] Validate ownership, tick windows, command ordering, and the legal turn
  rate server-side. Bound memory/work without silently dropping accepted input.
- [ ] Define keyboard queue behavior on focus loss, death, disconnect, reconnect,
  and prolonged synchronization stalls; avoid stale turns firing after recovery.

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

### Remaining late-command decision

- **Strict deadline:** reject commands received after their intended server tick.
  Preserves already-published history but can discard physical presses under jitter.
- **Bounded late execution:** apply late commands in order on subsequent ticks,
  at most one per tick, and explicitly reject expired commands. The proposed
  100 ms / 12-tick grace window is an unvalidated tuning candidate, not an agreed
  policy or supported-network guarantee. It cannot restore the intended corner
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
   Compilation is not execution: the repository's no-agent-build rule means
   runtime results require a user-run or approved external workflow.

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
- [ ] Record RTT, jitter, loss estimates, input starvation, input lead, queue
  depth/age, correction count/distance, rollback depth/cost, and bytes per second.
  Distinguish measured loss from application-level missing input.
- [ ] Measure key-event-to-predicted-turn and predicted-turn-to-visible-frame
  latency; separately measure authoritative acknowledgment. Do not label RTT
  or tick duration as total input latency.
- [ ] Add a deterministic headless server + two-client integration harness,
  using the real input/replication pipeline and scripted, seeded inputs.
- [ ] Include same-frame bursts, consecutive same-direction turns, mixed turns,
  frame hitches, catch-up ticks, rollback during input, and long-running sessions.
- [ ] Exercise joining/leaving during play, reconnects, server restart, late
  spawn replication, and missing opponent history.
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
- [ ] Verify incompatible protocol/build rejection, disconnect cleanup, and
  stale-session isolation.
- [ ] Document supported deployment topology, UDP/firewall requirements,
  diagnostics collection, and operational limits before public hosting.

## Repository and tooling additions to evaluate

Keep work in this repository. No new repository, dependency, or service is
required merely to start this backlog.

| Addition / candidate | Purpose and constraints |
|----------------------|-------------------------|
| Headless integration test target or small workspace harness crate | Real client/server sessions, seeded scripts, state/command assertions. Reuse shared simulation and production networking setup. |
| Development-only network impairment configuration | First inspect Lightyear 0.28 facilities; otherwise use a test-only UDP proxy. Must support reproducible bidirectional loss/delay/reordering. |
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

Repository instructions currently prohibit agent-driven builds. Runtime tests
and harness execution require an explicitly approved execution environment or
user-run workflow; `cargo check` / `cargo clippy` are not substitutes.

## Completion gate and documentation

- [ ] Execute existing gameplay/input regressions, then add protocol, collision,
  and end-to-end impairment regressions as fixes land.
- [ ] Establish and publish numerical acceptance budgets before calling the
  game competitive-ready; do not infer them from the framework or tick rate.
- [ ] Demonstrate no missing/duplicate accepted turns within the supported
  envelope, repeatable collision outcomes, bounded resource use, and controlled
  behavior outside that envelope.
- [ ] Update `docs\netcode-design.md` to distinguish implemented behavior from
  aspirational claims about quantization, input acknowledgment windows, time
  dilation, and conflict resolution.
- [ ] Keep `docs\dev.md` aligned with actual scenario commands and diagnostics.

Suggested order: command tracing and turn semantics, protocol regressions,
two-client impairment harness, collision fairness, then presentation/capacity
and production hardening. Do not delay instrumentation until the end.
