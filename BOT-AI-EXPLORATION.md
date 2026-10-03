# Tron Zero — Bot AI Exploration

Research compiled **2026-10-03** for improving bot intelligence, pathing,
competitiveness, and difficulty. This is a survey, not a commitment to
implement. It merges (a) an audit of the current Rust bots and the unported JS
bot stack, and (b) online research into game-AI techniques, papers, and the
Rust/Bevy ecosystem.

> `docs/core-gameplay-mechanics.md` remains normative. Nothing here proposes
> changing the simulation contract; bots only emit ordinary player inputs.

## 0. Current state (grounding)

**Rust (`crates/server/src/bot.rs`).** Three bots. `bot_brain_input` writes a
random `TurnLeft`/`TurnRight` every 15–30 ticks into `ActionState<PlayerInput>`;
`bot_spawner` auto-cycles every 30 s and respawns 2 s after death; `random_spawn`
places bots. There is **no sensing, no pathing, no strategy, no memory**. This is
the entire AI today.

**JS stack (unported, inventoried from `..\tron-zero-js`).** A layered,
heuristic-only engine — no A*, no minimax, no learning:
- `BotRaycastSensing` — 7-ray fan (front/left/right + 4 diagonals) over a spatial grid.
- `CorridorFreedom.measureFreedom` — capped 4-neighbour BFS reachable-area + cardinal exits.
- `TurnCandidateScorer` — scores hold/left/right under 4 modes: `ESCAPE`/`SURVIVAL`/`ATTACK`/`EXPLORE`.
- `EnemyThreatModel` — straight-line `projectPosition`, `computeTrapScore` = reduction of enemy BFS freedom (×1.4) + turn bonus.
- `BotStrategyWeights` — `CUT_OFF`/`BOX_IN`/`SPEED_DEMON`/`TRAPPER` additive biases.
- `BotMemory` — 8-sample ring of freedom/exits/entrapment for trend slope.
- `BotDegradation` — 3 tiers (FULL/T1/T2) from `ticksInBatch` (and an unused frame-time branch).
- Budget constants in `BotAiBudget.ts` (BFS budgets 800/400, radius 18, lookahead 4 ticks, thresholds).

**Known gaps in that design** (these are the real improvement targets):
1. **Coarse grid.** Default arena 2400×2400 with `SPATIAL_CELL_SIZE=320` → only **8×8 = 64 cells**. Every BFS budget/radius is non-binding; freedom is quantized to 320-unit cells. A finer grid or a continuous representation is needed.
2. **No goal-directed search.** "Pathfinding" is flood-fill + straight-line projection; `projectPosition` ignores walls, so trap scores are optimistic.
3. **Enemy model is naive.** Trap score assumes the enemy goes straight and does not react.
4. **One-step/shallow horizon** (lookahead 4 ticks) — no multi-turn plans, no chamber/territory reasoning.
5. **Redundant work** (ray fan recast per candidate, overlay rebuilt per candidate); no time-budget enforcement (`PER_TICK_BUDGET_MS` dead).
6. **Difficulty is not a design axis** — strategies are assigned at random, no skill grading, no player-relative tuning.

The opportunity: the JS stack is a good *baseline* (recovering parity is worth it),
but the research below shows several **cheap, high-leverage upgrades** that go
well beyond it.

## 1. Sensing and spatial representation

| Technique | Summary | Fit / cost |
|---|---|---|
| **Ray/sensor fan** | Multiple rays from the rider (front/left/right + diagonals); nearest obstacle per ray. Armagetron's `gAISensor` and the JS 7-ray fan. | Already the baseline; cheap. Keep. |
| **Coarse grid + BFS** | Uniform cell grid; traversability, reachable area, exits. | Cheap, but fix cell size vs arena. |
| **Continuous trail queries** | Keep the trail as a polyline; query nearest segment/range with an R-tree (`rstar`) instead of rasterizing every frame. | Removes 320-unit quantization; `rstar` nearest-neighbor ~1 µs. |
| **Layered influence maps** | Grids of propagated entity influence (threat, friendly, boost, region value), combined into a tactical field. Game AI Pro 2 Ch30/31. | **High ROI, cheap.** Multi-source BFS flood ≈ tens of µs on ~2.5 k cells. |

Armagetron reference: [`gAIBase.cpp`](https://wrtlprnft.net/doxy/html/gAIBase_8cpp-source.html)
(sensor scan, `CheckLoop` winding-number loop detection, `IsTrapped`);
[`gAISensor`](https://wrtlprnft.net/doxy/html/classgAISensor.html) (side/tunnel
scan + loop detection); state enum `AI_SURVIVE/AI_TRACE/AI_PATH/AI_CLOSECOMBAT`.

Influence maps: [Game AI Pro 2 Ch30 Modular Tactical Influence Maps](https://www.gameaipro.com/GameAIPro2/GameAIPro2_Chapter30_Modular_Tactical_Influence_Maps.pdf),
[Ch31 Spatial Reasoning](https://www.gameaipro.com/GameAIPro2/GameAIPro2_Chapter31_Spatial_Reasoning_for_Strategic_Decision_Making.pdf),
[Ch29 Escaping the Grid](https://www.gameaipro.com/GameAIPro2/GameAIPro2_Chapter29_Escaping_the_Grid_Infinite-Resolution_Influence_Mapping.pdf),
[Dishonored 2 flood-filled chase map](https://www.gameaipro.com/GameAIProOnlineEdition2021/GameAIProOnlineEdition2021_Chapter06_Flooding_the_Influence_Map_for_Chase_in_Dishonored_2.pdf).

## 2. Survival, search, and reachability

| Technique | Summary | Fit / cost at 120 Hz |
|---|---|---|
| **Capped BFS freedom** | Reachable area + exits from a cell; the JS core metric. | Cheap; keep but reduce cell size. |
| **Real-time heuristic search** — LRTA* | One-step lookahead + learned heuristic; must move each step. | Cheap per tick but needs a good heuristic; can be exploited. |
| **Incremental search** — LPA*, D* Lite | Replan only locally after edge-cost changes; D* Lite runs backward from goal. | Correct fit (static trails, moving rider) but full replans are ms-scale → reduced cadence. |
| **Anytime search** — ARA*, Anytime Dynamic A* | Weighted A* with shrinking ε; returns best-so-far on a deadline. | Clean "return best plan before budget" semantics. |
| **RHEA (Rolling Horizon Evolutionary)** | Evolve action sequences over a rolling window, execute first action, shift. | Powerful but ms-scale; macro-actions + warm start + reduced cadence. |
| **Macro-action planning** | Plan over turn points (branching ~3) instead of per tick. | **Structural key** — makes all search affordable. |

Sources: [D* Lite (Koenig & Likhachev 2002)](http://idm-lab.org/bib/abstracts/papers/aaai02b.pdf),
[LPA* (2004)](https://doi.org/10.1016/j.artint.2003.12.001),
[LRTA* (Korf 1990)](https://doi.org/10.1016/0004-3702(90)90054-4),
[Anytime Dynamic A* (ICAPS 2005)](https://cdn.aaai.org/ICAPS/2005/ICAPS05-027.pdf),
[RHEA SOTA](https://arxiv.org/abs/2003.12331).

**Space-filling / Hamiltonian (Snake).** A precomputed Hamiltonian cycle plus
shortcuts gives guaranteed non-self-collision (John Tapsell's Nokia Snake AI).
Directly **unsuitable** for live Tron: two riders mutate the graph continuously
and longest-path is NP-hard. Useful only as inspiration for "safe ordering"
reasoning. Sources: [Tapsell](https://johnflux.com/2015/05/02/nokia-6110-part-3-algorithms/),
[longest path NP-hard](https://en.wikipedia.org/wiki/Longest_path_problem).

## 3. Territory and trap evaluation (highest-value new idea)

This is the strongest upgrade over the JS "freedom + trap" proxy.

- **Voronoi territory.** Two-source BFS: assign each free cell to whichever rider
  reaches it first. The node surplus approximates who owns the map and doubles as
  an endgame detector.
- **Articulation points / biconnected components.** Trails cut the free space into
  **chambers**; the block-cut tree of that space is the natural decision graph.
  Hopcroft–Tarjan gives articulation points + blocks in linear time.
- **a1k0n's "Trap" (Google AI Challenge 2010 winner).** Compute Voronoi territory;
  find articulation points assuming the opponent's region is disconnected; DFS the
  current chamber counting nodes `N` and edges `E` while collecting adjacent cut
  vertices without crossing them; recurse. If the child chamber is **not a
  battlefront**, add its value (farmable); if it **is**, ignore the current chamber
  and add only the steps to enter. Return the max. Evaluation was data-mined from
  11,691 games: `score = K1·(N1−N2) + K2·(E1−E2)`, `K1=0.055`, `K2=0.194`. A
  **checkerboard parity bound** (each move flips square color) prunes the endgame.
- **Region adjacency graph (RAG).** Contract chambers to a small graph and run cheap
  minimax/utility on it.

Fit: directly portable to a rasterized arena and a strict superset of the JS BFS.
Cost: components + Tarjan ≈ tens of µs (feasible per tick / reduced cadence);
Voronoi Dijkstra is the cost driver (run every 6–12 ticks). Coefficients must be
re-fit to this arena.

Sources: [a1k0n post-mortem](https://www.a1k0n.net/2010/03/04/google-ai-postmortem.html),
[tronbot source](https://github.com/a1k0n/tronbot),
[biconnected components](https://en.wikipedia.org/wiki/Biconnected_component),
[SPQR tree](https://en.wikipedia.org/wiki/SPQR_tree),
[connected-component labeling](https://en.wikipedia.org/wiki/Connected-component_labeling).

## 4. Adversarial and multi-agent reasoning

| Technique | Summary | Fit |
|---|---|---|
| **Minimax + α-β + iterative deepening** | Discrete-turn Tron variant; flood-fill/Voronoi eval; kill-or-survive rewards; transposition tables + move ordering. | Strong baseline; needs macro-actions and a coarse eval. |
| **MCTS / MCTS-Solver** | Simulate to terminal; Solver proves wins. Evaluators (Voronoi, Tree-of-Chambers) improve play-outs but add cost. | ms-scale; reduced cadence / macro-actions. |
| **Decoupled UCT (DUCT)** | Independent UCB1 per agent at joint nodes; best in the Tron case study (62.3%). Plain UCT does **not** converge to Nash. | Reduced cadence. |
| **Regret Matching** | Probability ∝ positive cumulative regret; O(actions)/tick, converges to correlated equilibrium in self-play. | **Great cheap outer layer** to mix strategies. |
| **CFR / Deep CFR** | Iterative regret minimization; beats UCT in simultaneous games, but is a **training-time** algorithm. | Offline-trained policy, runtime lookup. |
| **Opponent modelling** | Predict opponent's next move from action history (MLP / Bayesian / n-grams); use in rollouts. | Cheap, high value. Extends the JS straight-line projection. |

Sources: [MCTS for simultaneous-move Tron (Winands et al., BNAIC 2013)](https://dke.maastrichtuniversity.nl/m.winands/documents/sm-tron-bnaic2013.pdf),
[Selection/play-out enhancements (Maastricht BSc)](https://project.dke.maastrichtuniversity.nl/games/files/bsc/Laschet_Bsc-paper.pdf),
[Endgame detection in Tron (Voronoi / Tree of Chambers)](https://project.dke.maastrichtuniversity.nl/games/files/bsc/Kang_Bsc-paper.pdf),
[UCT vs CFR in simultaneous games](https://webdocs.cs.ualberta.ca/~nathanst/papers/uctcfr.pdf),
[CFR (2007)](http://poker.cs.ualberta.ca/publications/NIPS07-cfr.pdf),
[Deep CFR](https://arxiv.org/abs/1811.00164),
[Opponent modelling in Tron using RL (ICAART 2018)](https://www.ai.rug.nl/~mwiering/GROUP/ARTICLES/ICAART_TRON_OPPONENT_2018.pdf).

## 5. Reactive local avoidance (lower priority)

- **Potential fields** (Khatib 1986) — attract to goal, repel obstacles; cheap but local minima/oscillation.
- **Dynamic Window Approach** (Fox/Burgard/Thrun 1997) — sample feasible `(v,ω)` under dynamics; **mismatch** because Tron has fixed speed + discrete turns.
- **Velocity Obstacles / ORCA** (Fiorini & Shiller 1998; van den Berg 2011) — reciprocal collision avoidance, fast and smooth, but assumes continuous velocity choice. For Tron it mostly reduces to anticipatory pairwise avoidance, projected onto 3 discrete actions, and handles the static trail only as obstacles.
- **Rust:** [`dodgy_2d`](https://crates.io/crates/dodgy_2d).

Sources: [ORCA project](https://gamma.cs.unc.edu/ORCA/),
[Game AI Pro 3 Ch19 RVO/ORCA](https://www.gameaipro.com/GameAIPro3/GameAIPro3_Chapter19_RVO_and_ORCA_How_They_Really_Work.pdf),
[Game AI Pro 2 Ch19 Anticipatory Collision Avoidance](https://www.gameaipro.com/GameAIPro2/GameAIPro2_Chapter19_Guide_to_Anticipatory_Collision_Avoidance.pdf).

## 6. Learning approaches (optional, offline)

- **RL** (Q-learning, PPO) on grid Tron works but needs millions of steps and a fast sim; Tron is non-stationary (same action → different outcome) which hurts vanilla RL.
- **Modular RL** splits non-stationary (interaction) vs stationary (isolated survival) phases; beats the a1k0n baseline on small grids and is faster than tree search.
  [Modular RL for Tron](https://exa.ai/library/publication/gx457hmpstf).
- **Self-play + league + curriculum** is the proven route for 2-player zero-sum (AlphaStar, OpenAI Five, TiZero). Data-expensive; ships via ONNX/`tract`.
- **Imitation / behavior cloning / DAgger** — BC from human replays suffers covariate shift; DAgger fixes it with expert queries. Best used for **personality bots**, not the default.
- Recommendation: keep heuristic bots as default; a learned "hard" bot is a later, separate track.

Sources: [AlphaStar (Nature 2019)](https://doi.org/10.1038/s41586-019-1724-z),
[AlphaStar Unplugged](https://arxiv.org/abs/2308.03526),
[OpenAI Five/Dota 2](https://arxiv.org/abs/1912.06680),
[DAgger](https://arxiv.org/abs/1011.0686),
[TiZero](https://ar5iv.labs.arxiv.org/html/2302.07515).

## 7. Architecture and decision selection

- **FSM** — current JS "mode" selection; breaks down as conditions multiply.
- **Behavior Trees** ([Isla, Halo 2](https://www.gdcvault.com/), `bonsai-bt`, `beet_flow`) — authored, hierarchical; good for readable personality structure.
- **Utility AI** ([IAUS, Dave Mark](https://www.gameaipro.com/GameAIPro/GameAIPro_Chapter06_The_Behavior_Tree_Starter_Kit.pdf), `big-brain`) — score candidate actions, pick max; fluid, designer-tunable curves.
- **GOAP / HTN** — runtime A* over actions; powerful emergent sequences but expensive and harder to debug; usually a module inside a BT, not the supervisor.
- **Hybrids dominate in production.** For Tron: keep the JS candidate-scoring loop (already utility-like), optionally add a BT/`big-brain` layer for strategy selection, and **stagger** the expensive parts.

Sources: [Beyond State Machines (Socratopia)](https://www.socratopia.app/library/game-code-anatomy-en/chapter-12),
[Choosing BT vs GOAP (Davide Aversa)](https://www.davideaversa.it/blog/choosing-behavior-tree-goap-planning/),
[BT survey (ScienceDirect)](https://www.sciencedirect.com/science/article/pii/S0921889022000513).

## 8. Difficulty and player experience

- **DDA / flow** — adjust to keep the player between boredom and frustration ([Zohaib review](https://doi.org/10.1155/2018/5681652), [Hunicke 2005](https://doi.org/10.1145/1178477.1178573), [Flow in Games](https://www.jenovachen.com/flowingames/Flow_in_games_final.pdf)).
- **Tune policy, not physics.** Prefer planning horizon, reaction delay, mistake probability, target greediness, strategy mix — visible knobs. Hidden speed/resource handicaps read as cheating.
- **Perception of AI** ([Game AI Pro 3 Ch1/Ch4](https://www.gameaipro.com/GameAIPro3/GameAIPro3_Chapter04_Player_Perception_of_AI_Opponents.pdf)) and **deliberate reaction time** ([Game AI Pro 2 Ch5](https://www.gameaipro.com/GameAIPro2/GameAIPro2_Chapter05_Agent_Reaction_Time_How_Fast_Should_An_AI_React.pdf)); L4D AI Director for pacing.
- **Personalities** — map `CUT_OFF/BOX_IN/SPEED_DEMON/TRAPPER` to legible styles with graded skill.
- **Rubber-banding** already exists mechanically (rubber + slide boost); prefer it to hidden bot buffs.
- Tron is lethal and >2-player zero-sum: one weak bot gets farmed. Grade difficulty across bots, and let the player survive longer rather than handing them free kills.

## 9. Time-budget and anytime engineering (critical)

Total fixed timestep is 8.33 ms, shared with the deterministic sim. Assume
**~0.5–1.5 ms/bot** for 3+ bots.

- **Stagger decisions:** run expensive reasoning for bot `i` on ticks where
  `tick % N == i` (10–20 Hz), cache the chosen action between epochs. This is the
  single most important pattern.
- **Hard node/iteration caps** instead of wall-clock timers (keeps tests
  deterministic). Never discard the last good plan mid-search; degrade to it.
- **Macro-actions** (plan over turn points) are the structural version of budgeting.
- Keep expensive work (`petgraph`, Voronoi, MCTS) off the hot path; keep all
  allocations out of the tick (flat `Vec<f32>` / CSR buffers as resources).
- `shared/` must stay wall-clock-free and deterministic; budget logic belongs in
  the **server** bot systems (they only produce inputs).

Sources: [Anytime algorithm](https://en.wikipedia.org/wiki/Anytime_algorithm),
[Game AI Pro Ch14 LOD Trader](https://www.gameaipro.com/GameAIPro/GameAIPro_Chapter14_Phenomenal_AI_Level-of-Detail_Control_with_the_LOD_Trader.pdf),
[Game AI Pro 3 Ch34 1000 NPCs](https://www.gameaipro.com/GameAIPro3/GameAIPro3_Chapter34_1000_NPCs_at_60_FPS.pdf).

## 10. Rust / Bevy ecosystem

| Crate | Use |
|---|---|
| [`big-brain`](https://crates.io/crates/big-brain) | ECS utility AI for strategy selection. |
| [`bonsai-bt`](https://crates.io/crates/bonsai-bt) / [`beet_flow`](https://crates.io/crates/beet_flow) | Behavior trees (if a BT layer helps). |
| [`petgraph`](https://crates.io/crates/petgraph) | `articulation_points`, `bridges`, `connected_components`, `tarjan_scc`, `dijkstra`, `astar` (no block-cut tree built in). |
| [`pathfinding`](https://crates.io/crates/pathfinding) | Generic BFS/DFS/A*/Dijkstra, `separate_components`. |
| [`rstar`](https://crates.io/crates/rstar) | R-tree for continuous trail segment queries. |
| [`dodgy_2d`](https://crates.io/crates/dodgy_2d) | ORCA reciprocal avoidance. |
| [`tract`](https://crates.io/crates/tract) (inference) / [`burn`](https://crates.io/crates/burn) / [`candle`](https://crates.io/crates/candle-core) | Optional learned policy; prefer `tract` (pure Rust, no libtorch) for shipping. |

Experimental Bevy AI crates (`bevy_behavior`, `bevy_thinker`) are new — treat cautiously.

## 11. Candidate roadmap (synthesis)

Ordered by leverage/cost. Effort is relative to the current Rust code (S/M/L).

| # | Candidate | Why | Effort | Key prerequisite |
|---|---|---|---|---|
| 1 | **Port the JS bot stack** (ray fan, BFS freedom, candidate scorer, threat, strategies, memory, degradation) | Baseline parity; testing-friendly; replaces random turns | M | A local grid for BFS, input plumbing per tick |
| 2 | **Fix grid resolution / add continuous trail queries** | Removes 320-unit quantization; makes all metrics meaningful | S–M | Decide grid vs `rstar` |
| 3 | **Chamber/territory evaluation** (Voronoi + articulation/block-cut + a1k0n battlefront score) | The biggest intelligence jump over "freedom" | M–L | Graph decomposition + coefficients re-fit |
| 4 | **Layered influence maps** (threat / region value / boost) | Unified cheap spatial reasoning; supports target selection | S–M | Flat grid buffers |
| 5 | **Staggered ticks + hard node caps + graceful degradation** | Required to fit 120 Hz with any real search | S | Server-side scheduler |
| 6 | **Macro-action planning + shallow α-β/MCTS** | Real look-ahead beyond 4 ticks | M–L | #5 |
| 7 | **Regret-matching strategy mix + opponent modelling** | Cheap adaptation; exploits predictable opponents | S–M | #1 |
| 8 | **Difficulty tiers & personalities** (policy knobs: horizon, reaction delay, mistake prob, strategy mix) | Player-facing value; no hidden handicaps | M | #1/#6 |
| 9 | **Optional: ORCA anticipatory avoidance** | Better head-on/pairwise avoidance | M | Project onto 3 actions |
| 10 | **Optional: learned hard bot** (self-play league, ship via `tract`) | Ceiling-raising, research track | L | Offline training infra |

Suggested first slice: **#1 + #5 + #4**, then **#2/#3**, then **#8**. That gives a
solid, cheap, tunable bot before any search or learning.

## 12. Caveats

- Online cost estimates are order-of-magnitude, not benchmarks; profile in
  `--release` before committing to MCTS/RHEA.
- a1k0n's coefficients and Tron grid research assume **discrete turn-based grids**;
  this game is continuous at fixed speed with cardinal turns. Port the *idea*, re-fit
  constants, re-validate.
- MCTS/CFR/self-play research is mostly turn-based; simultaneous continuous Tron
  invalidates some assumptions (non-stationarity, infinite state space).
- No technique here requires touching player-visible physics; keep bots
  server-only and input-emitting.
- Licensing: Armagetron (GPL-2.0-or-later) and a1k0n's MIT bot are references for
  behavior/architecture; this repo is Apache-2.0 — reimplement, don't copy source.
