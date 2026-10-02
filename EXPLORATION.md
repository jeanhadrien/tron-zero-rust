# Armagetron Advanced exploration

Source-grounded ideas for Tron Zero, investigated on **2026-09-30**.

Upstream: [ArmagetronAd/armagetronad](https://github.com/ArmagetronAd/armagetronad).
Snapshot: [`2186c8c145593800e1c030b379baffc781bd2ff8`](https://github.com/ArmagetronAd/armagetronad/tree/2186c8c145593800e1c030b379baffc781bd2ff8),
whose latest commit is dated 2026-09-13. All upstream references below are pinned
to that snapshot.

This is a source investigation, not a playtest or a commitment to implement
everything. Implementation code, configuration, schemas, and example maps were
examined; neither game was built or run. "Could implement" suggestions are our
design proposals, not claims about features already working in Tron Zero.

## Starting point and important differences

The Rust worktree was inspected, including its current uncommitted changes.
`PLAN.md` is not treated as proof of implementation. The original JS game's
implementation in `..\tron-zero-js` was also consulted.

| Area | Current Tron Zero baseline | What upstream adds |
|---|---|---|
| Movement | Continuous movement, four headings, one turn per tick; left/right input only | Brake reservoir, configurable turn delay and speed cost, more driving axes |
| Speed | Side proximity gives a slide boost; speed decays in open space | Distance-sensitive acceleration, wall ownership modifiers, tunnel/slingshot and breakaway boosts |
| Survival | Rubber drains while facing nearby obstacles and regenerates after escape | Different rubber models, grind preparation, gap policy, width-based damage |
| Trails | Finite-length `Trail(Vec<Vec2>)`, trimmed from the tail; cleared on death | Configurable dead-wall lifetime, unlimited walls, explosion holes, rubber-dependent length |
| Match structure | Connection-driven human spawning; bots respawn and rotate | Rounds, matches, scores, teams, ready-up, objective zones |
| Client | Basic 2D arena/cycle/trail rendering and a hard-follow camera | Gauges, minimap, spectating, camera choices, audio/visual feedback, split-screen |
| Bots | Random periodic turns in Rust | Sensor-driven survival, wall following, combat, pathfinding, personalities |
| Online systems | Lightyear prediction/replication and a localhost UDP connection | Discovery, favourites, chat, moderation, recordings, event logs |

Local entry points: `crates\shared\src\systems\player.rs`,
`crates\shared\src\components\player.rs`,
`crates\shared\src\components\trail.rs`, `crates\shared\src\protocol.rs`,
`crates\server\src\systems.rs`, `crates\server\src\bot.rs`, and
`crates\client\src\main.rs`.

**Some apparent additions are actually JS parity work.** The JS game already has
rubber/speed gauges, public chat, a room browser, spatial-audio listener code,
and a richer bot stack with `CUT_OFF`, `BOX_IN`, `SPEED_DEMON`, and `TRAPPER`
strategies. Its HUD explicitly reserves an unused brake slot: this is not
evidence of existing brake physics. Restore useful JS behavior before replacing
it with an upstream-inspired version.

**Do not overwrite the current feel by accident.** Upstream's rubber, speed
units, acceleration curves, and trail lifecycle differ from Tron Zero's. Keep
the existing rules as a default profile; introduce physics changes as explicit
presets. In particular, upstream rubber tracks usage, whereas our `Rubber`
tracks remaining capacity, so the meter conventions are opposite.

## Best candidates to pursue first

Effort is relative to the current Rust implementation, not a calendar estimate:
**S** = mostly localized work; **M** = several systems plus protocol/UI wiring;
**L** = new cross-cutting gameplay or geometry infrastructure. A small feature
can still depend on a large prerequisite.

| Candidate | Why it is valuable | Effort | Main prerequisite |
|---|---|---|---|
| Rubber/speed HUD and clear death feedback | Makes existing mechanics understandable immediately; restores JS parity | S-M | Local-player UI and confirmed event handling |
| Rounds, scoring, and scoreboard | Turns an endless simulation into a game with stakes | M | Authoritative match state and exactly-once death events |
| Brake/turbo reservoir | Adds a tactical input without adding weapons | M | Richer input representation and predicted resource state |
| Better bots | Makes solo testing useful and supports tutorials | M-L | Reuse JS sensing/scoring before adding upstream personalities |
| Map-defined walls and spawn points | Adds variety while keeping four-direction play | M | Validated arena data shared by collision and rendering |
| Sudden-death win/death zone | Prevents stalled rounds and introduces an objective | M | Rounds plus a basic authoritative zone system |
| Fortress and Sumo | Large gameplay variety from shared zone/team infrastructure | L | Teams, zone occupancy, capture progress, round outcomes |
| Physics presets | Enables experiments without breaking the default experience | M | Validated, replicated rules and round-boundary application |

## A. Movement, risk, and trail mechanics

### 01. Brakes with a reservoir; optional turbo

**Upstream:** Holding brake applies deceleration while a reservoir remains;
releasing it recharges the reservoir. `CYCLE_BRAKE`, `CYCLE_BRAKE_REFILL`, and
`CYCLE_BRAKE_DEPLETE` control this. A negative brake value turns the same input
into acceleration; the included single-use turbo preset disables refill.

**For us:** Add a holdable brake alongside turns, a predicted `BrakeReservoir`,
and a HUD gauge. Our current exclusive `PlayerInput` enum cannot represent
"turn left while braking"; use a combined input value. Brake, reusable boost,
and one-use boost should be distinct presets. **M.**
Sources: [acceleration implementation][physics], [turbo example][turbo].

### 02. Distance-sensitive and ownership-sensitive wall acceleration

**Upstream:** Acceleration increases as the cycle gets closer to a suitably
aligned wall, rather than being just an on/off boost. Self, teammate, enemy,
and rim walls have separate multipliers. `CalculateAcceleration` uses sensors
and the wall's direction/type to calculate the contribution.

**For us:** Turn slide boosting into a richer risk/reward curve in an opt-in
profile. Preserve wall ownership in sensor results, not just distance. Start
with self/enemy/rim; add teammate behavior once teams exist. **M.**
Sources: [physics, especially `CalculateAcceleration`][physics], [settings][settings].

### 03. Slingshots and tunnels

**Upstream:** Being between two nearby walls gets a separate acceleration
multiplier. A corridor involving your own wall is a slingshot; a corridor
without one is a tunnel. `CYCLE_ACCEL_SLINGSHOT` and `CYCLE_ACCEL_TUNNEL` allow
different treatment of the two.

**For us:** Reward deliberate corridor construction and risky overtakes.
Evaluate both side contacts and wall ownership together instead of applying two
unrelated boosts. This extends #02 and needs predictable corridor geometry. **M.**
Source: [slingshot/tunnel handling in `CalculateAcceleration`][physics].

### 04. Breakaway speed boosts

**Upstream:** Leaving a grind can change speed using wall-type-specific
`CYCLE_BOOST_*` and `CYCLE_BOOSTFACTOR_*` settings, with proximity affecting the
result. These settings are neutral in the inspected default configuration.

**For us:** An optional corner-exit boost creates another reason to time turns
carefully. Detect the transition away from a particular wall, not every tick
spent near it; retain that contact state through prediction/rollback. **M.**
Sources: [boost settings and description][settings], [movement implementation][physics].

### 05. Turn cadence and turning speed cost

**Upstream:** `CYCLE_DELAY` sets a minimum turn interval,
`CYCLE_DELAY_TIMEBASED` changes its time/speed relationship, and
`CYCLE_TURN_SPEED_FACTOR` reduces or modifies speed on turns. Pending-turn
memory is configurable too.

**For us:** Offer "classic" turning as a profile alongside our one-turn-per-tick
rules. Specify whether early turns queue, cancel, or expire; server and client
must apply identical tick-based eligibility. This changes tight zigzags,
double-turn escapes, and bots, not just input responsiveness. **M.**
Sources: [`GetTurnDelay` and movement code][physics], [turn settings][settings].

### 06. Rubber profiles and grind preparation

**Upstream:** Rubber behavior includes distance/time-based usage, refill
timescale, approach speed, minimum wall clearance, and reduced effectiveness
just after turns. `CYCLE_RUBBER_DELAY` and `CYCLE_RUBBER_DELAY_BONUS` can make an
unprepared turn into a wall especially dangerous.

**For us:** Explore low/high-rubber and preparation-sensitive modes, while
retaining the current cubic-speed drain as the default. Treat physics and HUD
as one change; do not translate upstream's used-rubber values directly into our
remaining-rubber meter. **M-L.**
Sources: [rubber settings][settings], [rubber handling][physics].

### 07. Open versus closed play: deliberate escape-gap policy

**Upstream:** Gap-related minimum-distance settings influence which openings a
cycle can squeeze through. Backdoor gaps can be treated differently from other
escape gaps. This is a ruleset choice, not just a collision epsilon.

**For us:** Define whether tiny gaps are legitimate escape routes before adding
more forgiving grinds. Provide named open/closed presets only after robust gap
detection; add fixtures for front gaps, rear escape gaps, recent corners, and
high-speed approaches. **L.**
Sources: [`CYCLE_RUBBER_MINDISTANCE_GAP*`][settings], [gap handling][physics].

### 08. Physical cycle width and narrow-corridor damage

**Upstream:** `CYCLE_WIDTH` and `CYCLE_WIDTH_SIDE` can make too-tight corridors or
too-close side grinds consume rubber or kill the rider. The implementation
checks both rear and front sensor pairs before applying squeezing damage.

**For us:** Consider a "wide cycles" or less pixel-perfect profile. Separate
visual size from collision clearance, show when a corridor is unsafe, and test
the interaction with rubber and own-trail junctions. The current rider is
effectively a point in the shared collision model. **L.**
Source: [width checks in `CalculateAcceleration`][physics].

### 09. Dead trails that persist, and configurable trail lifetimes

**Upstream:** Trail length and the time walls remain after death are
configurable. Dead walls can persist temporarily or indefinitely; nonpositive
length settings support unlimited-length behavior.

**For us:** Persistent wreck trails make deaths reshape the arena rather than
erase its history. Our trail is owned by the player and immediately cleared on
death, so separate trail lifetime from rider lifetime before enabling this.
Consider finite, unlimited, and short-lived wreck profiles. **M-L.**
Sources: [cycle wall lifecycle][cycle], [`WALLS_STAY_UP_DELAY`/length settings in game rules][game].

### 10. Explosions that cut holes in trails

**Upstream:** A death explosion can remove intervals of nearby player walls,
creating real escape holes. `S_BlowHoles` intersects the blast with walls and
limits cutting to geometry built before the explosion. Deadly explosions and
speed-dependent blast radii are separately configurable.

**For us:** This is one of the most interesting emergent mechanics: a death can
rescue a trapped opponent or open an attack route. Our continuous polyline
cannot represent a hole by merely deleting a vertex; introduce disjoint
segments/intervals shared by rendering and collision. Start with nonlethal
blasts; optional chain-killing blasts need separate balance work. **L.**
Sources: [blast/geometry implementation][explosions], [`BlowHole`][walls].

### 11. Rubber as health, expressed through trail length

**Upstream:** `CYCLE_RUBBER_WALL_SHRINK` subtracts used rubber times a factor from
the permitted finite wall length. The example health preset combines this
with nonreplenishing rubber to make the trail reflect remaining survival.

**For us:** An experimental attrition mode could shorten your territorial
footprint as you take damage. Couple trail trimming to authoritative rubber,
make the resource visible, and define whether healing restores only future
growth. The example configuration contains spelling mistakes; copy the idea,
not its lines blindly. **M**, building on configurable trail rules.
Sources: [`ThisWallsLength`][cycle], [health/trail example][health].

## B. Match formats and objectives

### 12. Rounds, matches, score limits, and victory presentation

**Upstream:** Last-survivor rounds feed into matches with score, round, and time
limits. There are also minimum-lead and mercy/blowout conditions, different
round-finish timings, winner announcements, and score summaries.

**For us:** Establish an authoritative lifecycle such as lobby/countdown,
playing, round result, and match result. Reset geometry and apply rules at
defined boundaries; handle ties and disconnects explicitly. This is a
foundation for nearly every competitive mode below. **M-L.**
Sources: [game state and match-winner analysis][game], [scoring settings][settings].

### 13. Kill credit from recent influence, not only the final wall

**Upstream:** Recent wall encounters contribute to an enemy-influence model.
The player influencing a victim can receive kill credit even when the victim
ultimately crashes somewhere else. Team, self, dead-player, and timeout rules
modify attribution; suicide/death/kill scores are distinct.

**For us:** Reward cutoffs and traps, not just direct trail impacts. First
implement simple final-obstacle ownership; then record recent meaningful
contacts and introduce assists/influence attribution with an understandable
death feed. Exactly-once scoring must survive rollback and simultaneous deaths.
**M-L.** Sources: [influence and death/scoring code][cycle], [attribution settings][settings].

### 14. Teams, balancing, and formation spawns

**Upstream:** Teams support membership, names/colours, locked rosters and
invitations, size limits, balancing rules, and formation offsets for wingmen.
Team identity also changes wall-acceleration and kill-attribution decisions.

**For us:** A simple two-team mode is a useful first step toward Fortress.
Replicate membership separately from colour, keep friendly trails physically
meaningful unless a preset explicitly says otherwise, and replace independent
random spawn placement with team-safe formations. **M-L.**
Sources: [team implementation][teams], [wingman/rules settings][settings], [game setup][game].

### 15. Warmup and ready-up

**Upstream:** Warmup has configurable minimum player count, ready-up reminders,
and respawn timing. Players can practice before the match begins; the game has
a separate warmup/pickup state model.

**For us:** Preserve today's fast respawn/testing loop as warmup rather than
forcing it into competitive rounds. Add readiness and a synchronized countdown,
with explicit handling for joining, leaving, and changing teams. **M.**
Sources: [warmup integration and respawn calls][game], [warmup state component][warmup].

### 16. Sudden-death win zones and death zones

**Upstream:** Round duration and time since the last death can trigger a zone.
Its position, initial radius, and expansion are configurable. Entering a win
zone can resolve the round; a death-zone variant kills instead.

**For us:** Prevent two cautious survivors from circling forever. Prefer a
simple, clearly telegraphed circle first; replicate the activation tick and
growth parameters. This is an objective hazard, not evidence of a full
battle-royale shrinking-arena mode. **M**, after #12.
Sources: [zone timing in game rules][game], [zone settings][settings], [zone effects][effectors].

### 17. Fortress: attack and defend bases

**Upstream:** A base's conquest progress increases with enemies inside and
decreases with defenders and decay. Capture can award points, kill some or all
owners, or decide a winner. A last-surviving-base variation is provided in the
"fortress soccer" configuration.

**For us:** Gives teammates attacker/defender roles without adding weapons.
Share a generic zone-occupancy system with a `TeamId`, capture progress, and
configurable outcome. Show contested/capturing/defended state in the HUD.
The soccer example describes territory rules, not a simulated ball. **L.**
Sources: [conquest calculation and outcomes][fortress], [soccer-style preset][soccer].

### 18. Sumo and team Sumo

**Upstream:** Sumo is built from fortress-zone rules: negative conquest decay
causes pressure toward collapse, while defenders counter it. The team-Sumo
preset combines team limits, capture kills, scoring, finite trails, and
rubber-dependent trail shrink.

**For us:** "Stay in and defend your zone while trapping opponents" adds
positional pressure to survival. Implement as a zone/team preset, not a second
physics engine. Clearly distinguish leaving a zone from immediate elimination:
the inspected rules use capture/collapse progress. **L**, mostly shared with #17.
Sources: [team-Sumo preset][sumo], [fortress progress and contact timeout][fortress].

### 19. Revival bases and rescue zones

**Upstream:** Base entry can respawn members of its owning team. Generic zone
effectors also include player spawning, and target selectors can select dead
players. Spawn invulnerability and delayed wall creation are configurable.

**For us:** A rescue objective gives living teammates a reason to take risks.
Define valid revive positions, cooldowns, protection, and trail treatment before
enabling repeated resurrection. The current bot auto-respawn does not provide
these multiplayer rules. **L.**
Sources: [base respawn logic][fortress], [spawn effectors][effectors], [spawn/protection settings][settings].

## C. Maps and reusable gameplay components

### 20. Map-defined walls, obstacles, and spawn points

**Upstream:** Maps define spawn locations/headings, wall polylines, driving
axes, ownership, zones, and settings. The repository includes obstacle maps and
fortress/tutorial maps, not just a rectangular boundary.

**For us:** Begin with a small validated map format for static segments and
spawns, keeping four headings. Both renderer and shared sensors should consume
the same data. Validate spawn clearance and wall bounds; use stable map
identifiers so client/server agree before play. **M.**
Sources: [map parser][parser], [map schema][map-schema], [obstacle-map example][obstacle-map].

### 21. More than four driving directions

**Upstream:** Maps can supply a number of axes or explicit direction vectors.
`eGrid` delegates heading/winding operations to `eAxis`, and configuration
examples describe six- and eight-direction arenas.

**For us:** Hexagonal or octagonal driving is a substantial variation while
retaining discrete turns. Replace cardinal-only heading assumptions, rotation
logic, spawn selection, and bot sensing together; test diagonal collinearity
and own-trail departure rules. Our current 90-degree component swaps are not
sufficient. **L.**
Sources: [axis component][axes], [grid direction API][grid], [arena examples][settings].

### 22. Animated circular and polygonal zones

**Upstream:** `zShapeCircle` and `zShapePolygon` support position, scale,
growth/collapse, and rotation over time. Shape state is networked, and maps can
define these parameters.

**For us:** Moving objectives, rotating hazard polygons, or growing resource
areas are plausible uses. Begin with circles evaluated from a replicated
reference tick; add polygon overlap only when gameplay needs it. These are
animated zones, not necessarily moving solid collision walls. **M-L.**
Sources: [shape types and time-dependent parameters][shapes], [map schema][map-schema].

### 23. Zone triggers and composable effects

**Upstream:** Zones distinguish entry, staying inside, leaving, and staying
outside. Effectors include win, death, points, brake recharge, rubber recharge,
acceleration, spawning, and setting changes. Selectors/validators determine
which players or teams receive an effect.

**For us:** A shared `ZoneShape` + trigger + target filter + effect model can
support recharge pads, boost lanes, checkpoints, or team-only hazards.
Those examples are proposed content, not all verified stock maps. Separate
edge-triggered effects from per-tick effects to avoid repeated points/refills;
keep outcomes authoritative and presentation rollback-safe. **L.**
Sources: [`InteractWith` and transition tracking][zones], [implemented effectors][effectors], [target/effect schema][map-schema].

### 24. Monitors: objective meters and rule graphs

**Upstream:** Monitors aggregate influences, apply drift and bounds, and execute
rules when values cross thresholds or fall in/out of ranges. Rules can affect
other monitors and zones.

**For us:** This is a reusable primitive for capture progress, charge meters,
multi-step objectives, or timed map events. Start with typed meters and a small
set of effects rather than a general scripting language; specify edge versus
level activation and prevent cyclic/unbounded effect chains. **L.**
Sources: [monitor model][monitors], [monitor/rule schema][map-schema].

### 25. Ruleset presets and map/config rotation

**Upstream:** Most mechanics are settings, and `MAP_ROTATION`,
`CONFIG_ROTATION`, and `ROTATION_TYPE` cycle content per round or match.
The repository also has optional Ruby round/match callbacks and a richer XML
rotation example.

**For us:** Expose curated profiles such as Default, Low Rubber, Turbo, and
Sumo rather than hundreds of loose knobs. Validate and replicate a rules
snapshot; apply changes at round boundaries and show the active profile.
Sequential rotation is implemented in `gGame`; the XML example alone should
not be taken as proof that its richer rotation path is enabled. **M.**
Sources: [rotation implementation][game], [optional callbacks][rotation], [build options][configure].

## D. AI, learning, and presentation

### 26. Sensor-driven bots and distinct personalities

**Upstream:** AI has survival, wall-tracing, path, and close-combat states,
including emergency behavior and loop/trap sensing. Characters parameterize
reaction, view range, tracing, combat, pathfinding, and other abilities.

**For us:** First restore the JS candidate scorer, corridor freedom, threat
model, and strategy biases instead of porting an entire older AI engine.
Then add bounded reaction delay and perception range for readable difficulty
levels. Tutorial bots may use authored paths; competitive bots should only
submit ordinary player inputs. **M-L.**
Sources: [AI state/decision implementation][ai], [AI interface][ai-header], [character configuration][ai-characters].

### 27. Tutorials and skill challenges

**Upstream:** Tutorials/challenges exercise navigation, grinding, survival,
speed kills, team starts, brake boost, and high rubber. They use specialized
maps/settings and explicit success/failure analysis.

**For us:** Teach one mechanic at a time: turn through gates, ride near a wall
to accelerate, survive a rubber stall, then escape an opponent. Build
scenario definitions with deterministic starting state and measurable goals;
they can double as regression fixtures for gameplay changes. **M**, after
map loading and usable bot control.
Sources: [tutorial implementation][tutorials], [bundled tutorial maps][tutorial-maps].

### 28. HUD gauges, scoreboard, and minimap

**Upstream:** Cockpits are data-driven compositions of gauges, labels, maps,
and camera views. Data sources include rubber, speed, brakes, ping, and
player/team scores. Servers can forbid selected cockpit data or the HUD map.

**For us:** Restore JS rubber/speed gauges first, then add brake state, match
timer, score, and a minimap. A full XML cockpit engine is unnecessary initially.
Decide information visibility per mode: drawing enemy resource values or a map
can change the competitive game even if it looks like a UI-only feature. **S-M.**
Sources: [cockpit data callbacks][cockpit], [minimap and restrictions][minimap].

### 29. Spectating and camera choices

**Upstream:** Cameras support internal, follow, free, smart, and custom modes,
glance directions, speed-sensitive positioning, and changing the watched
object. Some modes can be forbidden by the server.

**For us:** Add death spectating and target switching, then smooth follow,
look-ahead, and optional arena overview. Port the useful ideas into 2D rather
than assuming first-person 3D is required. Keep camera smoothing outside
simulation state; respect any mode-specific restrictions on spectator
information. **S-M** for 2D; **L** for a new 3D presentation.
Sources: [camera interface][camera-header], [camera behavior][camera], [camera settings][settings].

### 30. Speed audio, grind sparks, and explosion feedback

**Upstream:** Cycle engine pitch depends on speed and Doppler effects; spatial
mixing and self-sound suppression are configurable. Sparks and explosions have
their own rendering components, and grind visuals can communicate clearance.

**For us:** Make speed changes and rubber pressure perceptible without staring
at a meter. Reuse the JS audio intent, but use original or suitably licensed
assets. Deduplicate one-shot death sounds/particles during rollback; scale
spark intensity from real contact data rather than arbitrary proximity. **M.**
Sources: [cycle sound mixing][cycle], [spark component][sparks], [sound settings][settings], [explosions][explosions].

### 31. Local multiplayer and split-screen

**Upstream:** A client can host multiple players, and viewport configurations
include two-player and four-quadrant layouts with per-player assignment.

**For us:** Couch play could be valuable for a native client. Separate local
player identity, input bindings, camera, HUD, and network ownership; the current
single local-player camera and input assumptions must change. Browser gamepad
support and usable small viewports would need their own investigation. **L.**
Sources: [viewport layouts/assignment][viewports], [multiple-player client limits][settings].

## E. Online experience and engine lessons

### 32. Server browser, favourites, and friend discovery

**Upstream:** The browser supports discovery, ping/player information,
filtering/sorting, favourites, and friend-related matching. There are Internet
master-server and LAN browsing paths.

**For us:** Restore the JS room-manager browser first; add favourite rooms,
measured latency, active mode/map, and filters. Do not replace our manager with
the old master-server protocol just to gain similar UX. Friend discovery
needs stable identity and an explicit privacy policy. **M.**
Sources: [browser][browser], [favourites][favourites], [friend component][friends].

### 33. Team chat and quick communication

**Upstream:** There are team/public chat paths, configurable instant-chat
bindings, and enemy-message suppression while alive. The game also has a
chatbot that can steer while its owner is chatting.

**For us:** Restore ordinary JS public chat, then add team routing and quick
messages useful for attack/defend modes. Keep chat focus and cycle controls
unambiguous. Automatic steering while typing is an interesting accessibility
idea, but also a competitive rule change; do not enable it silently. **M.**
Sources: [player chat and instant-chat handling][players], [chatbot behavior][cycle], [chatbot settings][settings].

### 34. Moderation, permissions, and voting

**Upstream:** Voting supports several actions with timeouts, biases,
eligibility/access rules, and anti-spam controls. Team locks, authentication,
silencing, kicks/bans, and referee-style permissions are additional server
components.

**For us:** Start with authenticated admin roles, auditable kick/mute, and
rate-limited chat. Add map/mode votes only for allowlisted choices; avoid giving
players generic config or command execution. Borrow the permission model, not
the legacy authentication protocol wholesale. **M-L.**
Sources: [voting component][voting], [authentication component][authentication], [dedicated-server settings][dedicated-settings].

### 35. Recording and playback

**Upstream:** `tRecorder` archives tagged sections of data and supports strict
playback and desynchronization detection. Recording is broader than saving
turn inputs; it participates in the game's handling of external observations.

**For us:** Record authoritative snapshots/keyframes, inputs, seed, rules,
map identity, protocol/build version, and confirmed events. This enables
shareable matches and reproducible collision/netcode bug reports. Inputs alone
are not enough to promise deterministic native/WASM playback with our current
state-replication architecture. **L.**
Sources: [recorder interface][recorder], [recording configuration][settings].

### 36. Structured match events and persistent statistics

**Upstream:** `eLadderLogWriter` gives events names, field specifications, and
independent file/script output controls. Game, player, and fortress code emit
round/match/player/capture events; statistics track accumulated outcomes.

**For us:** Create a versioned confirmed-event stream for deaths, winners,
captures, and joins. It can feed the HUD, match history, analytics, and
tournament tooling without each system reverse-engineering state changes.
Specify ordering, stable identities, and deduplication; omit private connection
data from public logs. Persistent rankings are a later product decision.
**M.** Sources: [event writer][ladderlog], [match events][game], [statistics][statistics].

### 37. Spatial-query and topology infrastructure

**Upstream:** The `eGrid` is a geometry/topology structure with points, faces,
and half-edges; it supports inserting walls and local wall-range queries.
Sensors and pathfinding build on that geometry. An optional "topology police"
detects illegal crossings, but is disabled by default because false positives
are possible.

**For us:** Take the lesson of a shared wall-query abstraction, not a mandate
to port the half-edge engine. Keep our naive collision strategy until profiling
justifies an index; consider the JS spatial-query work first. Swept collision
for simultaneously growing trails/head-to-head movement is a correctness
prerequisite for richer geometry, not something a faster index solves.
**L** if an index/topology engine is needed.
Sources: [grid API][grid], [topology-police caveats][settings], [current collision limitations](docs/core-gameplay-mechanics.md).

### 38. Bounded lag forgiveness and network observability

**Upstream:** Lag compensation uses a limited, replenishing credit budget,
per-event caps, thresholds, and client smoothing. Physics also has ping-related
rubber and packet-loss tolerance settings; the configuration warns that some
forgiveness can enable cheating.

**For us:** Useful design references for fairness policy and diagnostic UI, not
code to layer blindly onto Lightyear. Measure late inputs, rollback depth,
correction size, and collision disagreements first. Any extra forgiveness must
be capped and server-controlled, and tested for advantage under deliberate
delay/loss. Do not assume a client-provided timestamp proves fairness. **L.**
Sources: [lag-credit implementation][lag], [physics networking settings][settings].

## Suggested implementation boundaries

The main reusable building blocks suggested by this investigation are:

| Building block | Responsibilities | Enables |
|---|---|---|
| Validated rules snapshot | Explicit preset, replication/version, activation tick | Brakes, physics profiles, rotation |
| Authoritative match state | Round phase, countdown, limits, winner, resets | Scoring, warmup, objective modes |
| Confirmed event stream | Stable IDs, ordering, exactly-once presentation | Death feed, scores, sounds, logs, recordings |
| Team membership | Identity, roster, colours, spawn groups | Team acceleration, Fortress, team chat |
| Arena definition | Static segments, bounds, safe spawns, content identity | Custom maps and tutorials |
| Zone occupancy and effects | Shape queries, entry/exit, filtered effects | Sudden death, Fortress, Sumo, recharge/rescue |
| Trail geometry with independent lifetime | Ownership, creation tick, holes, expiry | Persistent dead trails and blast openings |
| Wall query interface | Distance, ownership/type, segment identity | Richer acceleration, bots, eventual indexing |

These are proposed concepts, not existing types to copy into the codebase.
Keep deterministic/predictable gameplay in `shared`, server authority and
irreversible outcomes in `server`, and rendering/UI/audio in `client`.
Do not play effects or write permanent scores during speculative replay.

A practical sequence is: restore HUD and useful JS bots; add match state and
confirmed events; introduce replicated presets and brake input; support static
maps; add teams and simple circular zones; build Fortress/Sumo from those
components. Trail holes, extra driving axes, and gap/width policies are separate
high-risk experiments, not prerequisites for that sequence.

## Caveats and things not established by this exploration

- **Licensing:** this Rust repository is Apache-2.0; upstream is GPL-2.0-or-later
  as stated in its [license/source headers][upstream-license]. Use behavior and
  architecture as references for an original implementation; directly copying
  source or assets requires a separate licensing decision. Asset rights may
  differ from code rights.
- **Experimental/legacy code:** the inspected configure defaults enable zones
  v2 (explicitly labelled experimental) and disable deprecated zones v1.
  Optional Ruby scripting is disabled by default. Source presence is not a
  guarantee that every shipped binary enables every path. See [build options][configure].
- **Examples are not specifications:** some configs have typos, old comments,
  compatibility constraints, or external resource URLs. The settings comment
  saying no server supports respawn is contradicted by current respawn code;
  implementation is the stronger evidence.
- **No assumed CTF, weapons, jump, ball physics, or battle royale:** passing
  mentions of flags/goals or a file named fortress soccer do not establish a
  complete implementation of those modes. They are not included as verified
  stock features here.
- **Old networking is not our target:** preserve Lightyear and the intended
  manager/transport architecture. Protocol-version compatibility and historical
  client workarounds are background context, not an automatic porting backlog.
- **No default physics retuning:** this document adds ideas, not changes to the
  mechanics required by `docs\core-gameplay-mechanics.md`.

## Upstream source references

[physics]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gCycleMovement.cpp
[cycle]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gCycle.cpp
[settings]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/config/settings.cfg
[turbo]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/config/examples/single_use_turbo.cfg
[health]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/config/examples/health_is_wall_length.cfg
[explosions]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gExplosion.cpp
[walls]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gWall.cpp
[game]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gGame.cpp
[teams]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eTeam.cpp
[warmup]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eWarmup.h
[fortress]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/zone/zFortress.cpp
[soccer]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/config/examples/fortress_soccer.cfg
[sumo]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/config/examples/teamsumo.cfg
[parser]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gParser.cpp
[map-schema]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/resource/proto/map-0.3.1-c.dtd
[obstacle-map]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/resource/proto/Your_mom/repeat/repeat.map.xml
[axes]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eAxis.h
[grid]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eGrid.h
[shapes]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/zone/zShape.h
[zones]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/zone/zZone.cpp
[effectors]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/zone/zEffector.cpp
[monitors]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/zone/zMonitor.h
[rotation]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gRotation.cpp
[configure]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/configure.ac
[ai]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gAIBase.cpp
[ai-header]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gAIBase.h
[ai-characters]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/config/aiplayers.cfg.in
[tutorials]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gTutorial.cpp
[tutorial-maps]: https://github.com/ArmagetronAd/armagetronad/tree/2186c8c145593800e1c030b379baffc781bd2ff8/resource/proto/AATeam/tutorials
[cockpit]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/cockpit/cCockpit.cpp
[minimap]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/cockpit/cMap.cpp
[camera-header]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eCamera.h
[camera]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eCamera.cpp
[sparks]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gSparks.cpp
[viewports]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/render/rViewport.cpp
[browser]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gServerBrowser.cpp
[favourites]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gServerFavorites.cpp
[friends]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gFriends.cpp
[players]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/ePlayer.cpp
[voting]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eVoter.cpp
[authentication]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eAuthentication.cpp
[dedicated-settings]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/config/settings_dedicated.cfg
[recorder]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tools/tRecorder.h
[ladderlog]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eLadderLog.h
[statistics]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/tron/gStatistics.cpp
[lag]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/src/engine/eLagCompensation.cpp
[upstream-license]: https://github.com/ArmagetronAd/armagetronad/blob/2186c8c145593800e1c030b379baffc781bd2ff8/COPYING.txt
