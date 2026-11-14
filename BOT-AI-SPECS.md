# Bot AI — Design Spec

Brainstorm captured 2026-10-03. This is a **living design doc**: it records what
we've settled, the reasoning behind it, and the decisions still open. It is not
a commitment to scope — it's the north star for the bot AI work.

> Core thesis: the simulation is deterministic, the bot is server-side, and a bot
> has perfect state and zero latency. Left alone, an AI can fork the world and
> become an oracle that only dies when geometry forces it. **Bot AI is therefore
> not "how do we make it smart" — it's "where do we amputate it so it stays
> fun."** Every section below is a version of that question.

---

## 1. The core model — Budget × Direction

Difficulty and personality are separate axes, and the opponent is their product:

```
Difficulty  = BUDGET     (how good the bot is allowed to be)
Personality = DIRECTION  (what the bot wants)
Opponent    = BUDGET × DIRECTION
```

- **Budget** — reaction latency, lookahead depth, perception radius, decision
  cadence, inference depth, error injection. Tuned to the player/server.
- **Direction** — the personality vector. Determines how the budget is spent.
- The bot does **not** throw matches. "Good time" is achieved by budget-matching
  and personality variety, **not** by outcome fudging.

**Difficulty is capability, not cooperation.** A weak bot can still be aggressive
and fun; a strong bot can still be a show-off.

### The personality vector *is* the objective function

Do not build a fixed "win" objective plus a separate behavior modifier. Make the
vector the reward that the planner maximizes:

```
value = w_survival · survival
      + w_kill     · kill_pressure
      + w_control  · area_denied
      + w_style    · pattern_score
      − w_risk     · exposure
      ...
```

One code path, infinite characters. A grinder has high `w_survival`, low
`w_risk`. A cutter has high `w_kill`, high `w_risk`. An artist has high
`w_style`. Critically: **these weights can be evolved offline via self-play** to
discover interesting personalities — the useful form of self-play, without a
neural net / black box.

### Settled

- [x] Difficulty is capability, tunable.
- [x] Bots genuinely try to win; fun comes from budget/personality, not fudging.
- [x] Willing to ship a bot that beats the author ~9/10 at max budget.
- [x] Personality = objective weights; a vector, expressive (embrace complexity).
- [x] Offline self-play may be used to calibrate/evolve the weight space.

---

## 2. Perception

Bots see the **same kind of view as the player**: top-down, the visible grid
region. Because the arena is a grid, "the window" is a rectangle of cells.

The perception model must be defined in **world/grid units**, independent of
camera zoom or resolution — *not* literally the render rect, or a player zooming
out would hand bots omniscience.

### Memory — required

The bot must not be stateless. Give it a **decaying occupancy memory**: cells it
has observed stay known with a confidence that fades. Trails persist visually
and humans remember them; a stateless bot looks brain-damaged. Memory depth is a
difficulty knob.

### Observation vs inference

- **Observation** — things currently inside the detection region (rides, trails).
- **Inference** — extrapolated futures of opponents.

Inference is the most "unfair-feeling" lever we have and must be an explicit
budget item. Seeing a rider is fair; predicting its turn is god-tier.

### Open

- [ ] Is perception stateless-per-tick or memory-backed? (Strong recommendation:
      memory-backed.)
- [ ] Detection region shape/size as a function of budget (grid cells).
- [ ] Does the bot infer opponent positions/turns, or only observe? Depth?

---

## 3. Latency

Two *different* latencies that compound. Model both.

| Layer | Meaning | Human analogue |
|---|---|---|
| **Reaction latency** | perceive → decide | ~200 ms |
| **Network latency (simulated ping)** | decide → takes effect | real ping, in ticks |

A search-based bot **must plan from a stale world state** *and* schedule its
input K ticks into the future. Humans do this implicitly; the bot won't unless
forced. If bots can skip committing to turns that land where the world *used to
be*, they aren't playing this game.

### Settled

- [x] Add reaction latency.
- [x] Add simulated ping (ticks).
- [x] Cap input rate (one turn/tick is the engine cap; humans can't sustain it).

### Open

- [ ] Exact reaction/ping values per budget tier.
- [ ] Does the planner explicitly account for its own input delay? (Should.)

---

## 4. Planner architecture

Two complementary techniques, split by role:

- **Analytic geometry → generates candidate actions.** Cut here, box that corner,
  grind this wall, retreat. Expresses Armagetron-style tactics cleanly and
  scales. Cheap.
- **Shallow world-clone rollouts → score the shortlist.** Does this line survive
  30 ticks? Does it force the opponent into a bad cell?

**Analytic = breadth, clone = verification.** Cloning for breadth will drown;
analytic-only will miss emergent corners. Do not clone Bevy ECS world state per
candidate per tick per bot at depth — that is the combinatorial wall.

### Open

- [ ] Define the generate→score seam precisely (candidate count, rollout depth).
- [ ] Compute budget per tick / rolling window.
- [ ] Replan cadence + commitment (see below).

### Replan cadence & commitment

A search bot re-planning every tick dithers at the margin. Enforce a **minimum
commitment window** (or replan only on invalidation). Saves compute *and* is a
"deliberate vs. twitchy" personality axis.

### Decision rate as a character trait

Deciding every tick is expensive and superhuman. A 3–8 tick decision cadence is
both a performance optimization and a personality axis. Fold into budget.

---

## 5. Tactics & the turtle problem

Bots should use **all mechanics**: boxing, cutting across the nose, baiting,
trapping, wall-grinding.

### The turtle problem

Trails clear on death, so nothing accumulates. In a large arena the *locally
optimal* play is to circle in open space forever — unwatchable. The value
function decides this: pure `w_survival` turtles. Fixes to consider:

- Every combat personality has nonzero `w_kill`.
- A time-decaying `w_survival` — the longer the match, the harder it must engage.

### Rubber — first-class

Rubber drains facing an obstacle, regens in the open, and enables slide boost.
If the planner's rollouts or heuristic ignore rubber drain / wall proximity, the
bot is either too timid to use the mechanic or misjudges recovery and dies
stupidly. **Rubber must be a first-class term in the value function.**
(Implemented: `Terms::rubber`, the burn of a line as a share of the rubber left, with a per-personality price; a target low on rubber is preferred as prey.)

### Settled

- [x] PvP focus.
- [x] Use the full tactical kit.
- [x] Rubber courage is tunable (per-bot risk appetite).

### Open

- [x] Forced-engagement pressure: a round heat (fight terms and risk up, room and
      margin down as the round ages), only for bots in a round.
- [ ] Spawn behavior: hunt on respawn / grace window / leave spawn. (Must not
      insta-kill fresh spawns.)
- [ ] FFA target selection: nearest / most dangerous / random / coalition-aware.

---

## 6. Opponent modeling

A cut only lands if the prediction of the opponent is right.

| Model | Cost | Enables |
|---|---|---|
| Assume-straight | trivial | little; exploitable |
| Assume-turn-rate | low | leading a target |
| Habit tracking (adapt) | higher | baiting *you* specifically |

Habit tracking is the payoff that makes bots feel intelligent — "oh shit, it
learned me." Treat it as a **budget item** (lower tiers don't get it).

### Settled

- [x] Adaptation via events (repeated kills, repeated patterns).
- [x] Bots can adapt within a match.

### Open

- [ ] Default opponent model per budget tier.
- [ ] Which events are tracked, and for how long.

---

## 7. Personality, identity, presentation

- **Vector, expressive.** High-dimensional, because we want complexity.
- **Character via style**: movement style, naming, chat behavior.
- **Stable base vector + transient mood layer** that decays back to base. Resolve
  the tension between "adapts" and "get to know them": it reacts, remembers a
  grudge, and returns to itself.
- **Tells** are cheap in principle — a visible wind-up before committing.
  Implement as "deliberately suboptimal commit + partial motion." The hard part
  is *consistency*: a tell that always fires becomes an exploit, and a smart bot
  would stop telling once it's exploited. **Tells and optimality are inversely
  related → disable tells above a difficulty threshold.** Another budget knob.

### Settled

- [x] Personality vector, make it expressive.
- [x] Character through style, naming, chat.
- [x] Adaptation via events.
- [x] Tells: like the idea (accept the implementation difficulty).

### Open

- [ ] Base+decay-mood architecture confirmed?
- [ ] Tell taxonomy and the budget threshold where they disappear.

---

## 8. Coalitions & chat

### Gangs / team-ups

Can form with chat callouts. **Watch the dogpile**: in FFA the human is often the
biggest single threat, so naive coalition logic gags every bot up on the player.
Needs a threat budget / anti-dogpile bias.

Coalition depth has a huge range:
- **Floor:** non-aggression (don't cut your ally).
- **Ceiling:** coordinated pincers (shared perception + timed two-sided traps).

### Chat

- **Preferred:** templated **intent-narration** — the planner emits intent tags
  and the chat layer renders them ("Cutting left." / "Grinding." / "You're
  predictable."). Cheap, personality-rich, inherently honest.
- **Tempting but costly:** LLM chat. Paying GPU to talk, not play. Not ruled out.
- Bots **can lie** — chat can be a tactical feint.

### Settled

- [x] Gangs/team-ups possible, with chat callouts.
- [x] Bots can lie.
- [x] Bots are **tagged as bots by name** (no impersonation of humans).

### Open

- [ ] Coalition floor vs. ceiling for MVP.
- [ ] Chat implementation: templated intent-narration vs. LLM.
- [ ] How much coordination (shared perception/plans) is in scope.

---

## 9. Difficulty, naming, persistence

### Naming collision

"Name encodes difficulty" fights "name encodes identity."
`Bot-3` is honest, breaks immersion. `Vex the Grinder` is immersive, hides
difficulty. Options: a title format encoding both, or hide difficulty and let
players infer.

### Who sets difficulty?

Player menu / server default / **adaptive** (track win rate, nudge budget).
Adaptive is powerful and can feel like rubber-banding.

### Persistence

Do bot identities persist across matches — stats, grudges, memory that *you*
killed them last game? Big retention lever; big state/storage question.

### Settled

- [x] Named with a difficulty label.
- [x] Stats: later / TBD.

### Open

- [ ] Naming format (difficulty × identity).
- [ ] Who sets difficulty; adaptive or not.
- [ ] Cross-match persistence in/out.

---

## 10. Degeneracies — intended vs. bug

Some degenerate behavior is **a feature** (aesthetic twitch, style quirks). Some
degrades gameplay and netcode (indecision oscillation → bad client interp,
unreadable play). The commitment window matters regardless.

### Open

- [ ] Explicit list of intended degeneracies vs. bugs to prevent.
- [ ] Symmetry breaking for mirror matches (identical bots on symmetric spawns
      can lock into identical loops/stalemates).
- [ ] Anti-clump / anti-suicide smoke checks.

---

## 11. Test harness (do this before twelve knobs exist)

Without it, tuning bots is superstition. Minimum viable:

- Headless bot-vs-bot tournament on a fixed seed set.
- Per-personality-pair win matrices.
- Behavior snapshots: does this vector turtle? suicide? jitter?

Define "the bot regressed" before the knob surface explodes.

### Settled

- [x] `rand` allowed in bot decisions; no replay requirement.
      (Note: `rand` must live *outside* the deterministic shared
      `simulate_players` core — bot decisions run before/around the sim, and
      must never desync predicted state.)

---

## 12. Recommended MVP spine (scope order)

1. Perception (grid window + decaying memory).
2. Latency (reaction + simulated ping).
3. Analytic planner (cut / box / trap primitives) + shallow clone scoring.
4. A **~5-knob** personality vector
   (aggression, risk, patience, precision, showmanship).
5. One ambient "artist" behavior.

That alone is a credible game. Later layers, in order:
chat → gangs → adaptation → offline self-play calibration.

**"Tunable" is not free.** Every knob is a documented, validated, balanced
public surface; QA explodes combinatorially. Decide which knobs are
player-facing vs. dev-only vs. derived.

---

## 13. Ambient / artist bots

A bot drawing spirals/patterns is not a weak fighter — it's a **separate
category**: ambient content, a moving hazard, scenery. Name the split
(combat bots vs. ambient bots) or you'll keep jamming an artist into a fighter
build and get something that does neither. Patterns also serve as a
**communication channel** (chill / bored / baiting).

### Settled

- [x] Want bots that paint pretty patterns with their tails.

---

## 14. Open questions — priority order

1. **Objective function**: confirm `value = Σ weights · terms` with the
   personality vector as weights. Which terms exist?
2. **Perception**: memory? inference depth? grid region shape?
3. **Planner**: generate→score seam, candidate count, rollout depth.
4. **Opponent model** default per tier; habit-tracking events.
5. **Forced engagement** vs. accepting passive bots.
6. **Scope**: is any of {chat, gangs, patterns, self-play} a day-one pillar, or
   all layers on the MVP spine?
7. **Who sets difficulty**, and is it adaptive?
8. **Persistence** of bot identity across matches.
9. **Coalition depth** floor/ceiling for MVP.
10. **Chat**: templated vs. LLM.
