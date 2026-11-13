# Generative audio — design proposal

Status: proposal, 2026-11-08. Nothing here exists in code yet. Open questions
at the end block the plan.

The idea: **no recorded samples**. Every sound is synthesized live from
simulation state, so what you hear is a second readout of the game: speed,
stored boost, rubber pressure, grinding, who is near, how crowded the arena
is. The ambience is a single generative piece that the whole arena plays
together, and every rider is an instrument in it.

## 1. Push-back first

- **Readability beats music.** Gameplay cues (rubber strain, grind, a turn,
  a death nearby) must be instant, unquantized and always on top. Musical
  ideas (shared harmony, tempo) may color those cues but never delay them.
  The mix has a fixed ducking hierarchy:
  own danger > own grind/turn > nearby opponents > far opponents > ambience.
- **The danger window is short.** At base speed rubber kills in about 22
  ticks (~180 ms), ~14 ticks at the cap. A "countdown" cue is useless at that
  scale. Rubber audio is about *pressure* (you are in the zone, how spent you
  are), not a timer. Output latency counts: 40 ms of buffering is a quarter
  of the window.
- **The web build pins the architecture.** wasm runs single-threaded and GitHub
  Pages can't send COOP/COEP (`docs/specs/web.md`), so no shared-memory
  audio thread is possible. Any engine that renders DSP inside the cpal/Web
  Audio callback on the main thread will glitch on rollback-heavy frames. See
  §4.
- **The client can't know some things.** `DeathLog` is server-only (a client
  must never accumulate it). Death *cause* (wall, own trail, someone's trail)
  is not replicated. Neither is grind *side*, nor whose trail is being ground.
  Everything below is either derived from replicated state or explicitly
  marked as an inference. No protocol change (in line with `NOTES.md`).

## 2. What the simulation gives us

Replicated to every client (`shared/src/protocol.rs`), per rider:

| Signal | Component | Notes |
|---|---|---|
| Position, heading | `Position`, `Direction` | step-interpolated for opponents |
| Realized speed | `SpeedMult` | after the wall slowdown |
| Stored boost | `TargetSpeedMult` | inertia; `target - speed` = "held back" |
| Velocity | `Velocity` | Doppler for free |
| Rubber | `Rubber` | 0..`BASE_RUBBER` |
| Grinding | `IsSliding` | bool; side and owner not replicated |
| In rubber zone | `IsColliding` | bool |
| Alive, life | `IsAlive`, `LifeGeneration` | life tag = dedupe key |
| Trail | `Trail` | polyline; length, segment count |
| Identity | `PlayerColor`, `RiderShape`, `PlayerId`, `BotDebug` | timbre seed |

Client-only: own turn presses (`input/keyboard.rs`, `TurnOutbox`), turn verdicts
(`TurnVerdict` with `TooLate`, `NotEligible`), the camera, and `Rendered`
render poses.

**Timing caveat.** Opponents' plain-replicated flags (`IsSliding`, `SpeedMult`)
arrive at the confirmed tick, while their pose is interpolated behind it.
`render/fx.rs` already reads them without compensation, which is fine for
sparks. Audio uses the same approach, and so is up to one interpolation delay
early. That is acceptable for continuous layers but not for one-shots, which
are keyed on interpolated `IsAlive` (see §5).

## 3. Architecture

```
              client (Bevy, Update, after RenderSet poses)
 ECS state ──► audio::observe ──► SoundFrame (control data, ~60 Hz)
                     ▲                    │
   PresentationEvents┘                    ▼
   (shared with fx)              AudioBackend trait
                                  ├─ native: cpal stream thread
                                  ├─ web: AudioWorklet + DSP wasm
                                  └─ lab: offline render to WAV
                                          │
                               crates/audio (pure DSP, no Bevy)
```

### 3.1 `crates/audio` (new lib)

A pure synthesis crate: no Bevy, no `web-sys`, no I/O, no `rand` (a local
xorshift like `fx.rs`). It holds:

- `SoundFrame`: the full control state for one moment. Per audible rider
  (capped, nearest N), the listener pose, global arena scalars, and a
  small list of one-shot `Cue`s with an offset in samples. Plain `Copy` data,
  postcard-serializable (it crosses into the worklet).
- `Engine::render(&mut self, frame: &SoundFrame, out: &mut [f32])`: the voices,
  the ambience, the mixer and a limiter. All parameters are slewed inside the
  engine (one-pole smoothing), so a 60 Hz control stream with jitter or a
  rollback step never produces clicks.

Module rule: only `client/` and `lab/` depend on it. `shared/` and `server/`
never do. The sim stays audio-free.

Why a standalone crate instead of client code: the same DSP has to compile
into three hosts (native thread, a separate worklet wasm, the lab). It is also
the cheapest unit to test (render N frames, assert RMS, no NaN, no DC).

### 3.2 Client `audio` module

- `observe` system (`Update`, after render poses, beside `emit_fx`): builds the
  `SoundFrame` from ECS state and hands it to the backend. Read-only.
- **Presentation events, shared with fx.** `fx.rs` already keeps a `seen` map
  to detect deaths and spawns. Factor that into one small detector that emits
  `RiderDied { entity, life }` / `RiderSpawned { .. }` keyed on
  `(entity, LifeGeneration)` transitions, consumed by both fx and audio. Sparks
  and sounds can then never disagree, and rollback replays can't fire a death
  twice for the same life.
- **Own turns come from the input path**, not from `Direction` changes
  (rollback replays those): a cue fires when a press is queued. A `TooLate`
  verdict fires a distinct "dropped turn" cue: cheap, honest feedback about
  latency that players currently can't perceive.
- Settings: master, effects, ambience, mute when unfocused; persisted in the
  client config next to `FxSettings`.

### 3.3 Lab: hear a scenario offline

`lab` gains `--audio out.wav`: it runs a scenario, builds a `SoundFrame` per
tick from sim state, renders it with the same `Engine` and writes a WAV (via
`hound`), plus an optional spectrogram PNG next to the existing trace PNG.

This only works if `SoundFrame` is built from sim terms (position, speed,
rubber, flags, life transitions) and not from `Rendered`. The client then has
a thin adapter that substitutes render poses for positions. The payoff:
sound design iterates on deterministic scenarios ("grind along a wall at
2.5x then hit the rubber zone") without launching the game. An agent can
check a spectrogram the same way it checks a trace PNG today.

## 4. Backend: the one real fork

| Option | Native | Web | Cost |
|---|---|---|---|
| **A. `bevy_seedling` (Firewheel)** with custom nodes | Excellent: Bevy 0.19 ↔ 0.8, audio graph, custom nodes, sample-accurate events, optional HRTF | Multi-threaded `web_audio` needs COOP/COEP + nightly → impossible on Pages. Falls back to cpal's main-thread Web Audio host. | Lowest code; web quality at risk |
| **B. Own thin host** (cpal native, AudioWorklet web), DSP in `crates/audio` | cpal callback on its own thread; controls through an `rtrb` SPSC ring | DSP compiled as a **second, tiny wasm** that runs inside an `AudioWorkletProcessor` on the browser's audio thread. No shared memory, no headers; `SoundFrame`s go over `MessagePort`. | More plumbing; best web behavior; main bundle stays small |
| **C. `bevy_kira_audio` / kira** | Good, sample-centric; custom `Sound` is possible but not the model. The effects API arrived with Bevy 0.20. | cpal on the main thread, same issue as A | Wrong shape for synthesis |
| **D. Native Web Audio nodes** via `web-sys` (oscillators, biquads, panners) | n/a | Runs off the main thread with no worklet | Two sound implementations to keep in sync; rejected |

**Recommendation: B, in two phases.**
1. `crates/audio` + cpal native host + lab offline render. This proves the
   sound design.
2. The web worklet host. This is the expensive part:
   - a second wasm artifact built by both `web/build.ps1` and `build.sh`;
   - raw `extern "C"` exports with no wasm-bindgen glue, because
     `AudioWorkletGlobalScope` lacks `fetch`, `TextDecoder` and friends;
   - bytes fetched on the main thread and posted to the worklet;
   - a ~60-line JS processor;
   - the `AudioContext` resumed on the first user gesture (the menu click).

   Latency is one 128-frame quantum (~2.7 ms) plus the post hop.

A stays the fallback if web audio is deprioritized. Its native side is better
than anything we would hand-roll.

### DSP toolkit inside `crates/audio`

- **`fundsp` 0.23** (MIT/Apache): oscillators, SVF filters, noise, reverb,
  `Net` for hot-swappable graphs, `Shared`/`var` for atomic params. Use
  `default-features = false` to drop `files`/symphonia. To verify before
  committing: a clean `wasm32-unknown-unknown` build with no std-only paths
  that a worklet can't satisfy, the compile-time hit on the client (it is
  generic-heavy), and the worklet wasm size.
- Fallback: hand-rolled PolyBLEP oscillators + Chamberlin SVF + a small FDN
  reverb. About 400 lines, zero deps, trivially wasm-safe. This is
  reasonable if fundsp's compile cost or size disappoints.

Not considered further: FMOD/Wwise (licensing, heavy wasm, sample-first),
Glicol (a live-coding language, more than needed), `bevy_audio`/rodio (no
real synthesis story).

## 5. Sound design: mappings

All per-rider voices derive their timbre from identity: `RiderShape` picks
the oscillator family (Circle sine, Triangle triangle, Square pulse,
Diamond FM bell, Hexagon additive organ, Chevron sync saw). `PlayerColor`
hue picks the rider's root scale degree. Riders are recognizable by ear.

### 5.1 Own rider (always centered, never attenuated)

| Sim signal | Sound | Why it reads |
|---|---|---|
| `SpeedMult` 0.8–3.0 | Engine fundamental, log-mapped over ~1.5 octaves, plus brightness | "how fast" without looking |
| `TargetSpeedMult - SpeedMult` | Second oscillator at the *target* pitch; the gap is audible as **beating/detune** | You *hear* stored boost held back by a wall slowdown; it resolves when you break free |
| `IsSliding` | Band-passed granular sizzle; grain density ∝ speed. Panned to the grind side and tinted with the trail owner's timbre (own rider only, see note) | grind = reward sound |
| `IsColliding` | Rising filtered-noise strain + a sub "thump" at zone entry | danger onset, instantly |
| `Rubber` level | Engine timbre thins and gains crackle as rubber is spent; heals audibly with regen | "how fragile am I" across the next few seconds |
| Turn press | Short transient with a 5% downward pitch blip (the turn costs 5% cruise) | ties the sound to the mechanic |
| `TooLate` verdict | Dull muted click | latency becomes perceptible |
| Death (own life ends) | Engine voice shattered into grains, falling sweep, ambience ducks | — |
| Spawn | Rezz chord on the rider's root | — |

*Grind side and owner* are not replicated. For the own rider only, derive
them with the shared physics sensing helpers over the same obstacle set the
client already builds for prediction. That is one scan per frame. Opponents
get the plain centered sizzle.

### 5.2 Opponents and bots (spatial)

- Pan from listener-relative x (the camera follows the own rider). Gain and a
  low-pass from distance. **Doppler** from relative `Velocity` (riders reach
  1080 u/s, so a head-on pass is audibly dramatic).
- Same engine/sizzle layers, simplified. A voice cap of the nearest 6;
  farther riders fold into the ambience as a density term.
- Nearby death: the victim's timbre bursts, spatialized. Cause is not known.
  A plausible local inference ("the head ended on X's trail") could tint it,
  but it is only an inference and is never presented as fact.

### 5.3 Ambience: the arena as one piece

A slowly evolving harmonic field. All rider voices quantize their *timbre
partials* (not their engine pitch, which stays continuous for readability)
to the current chord, so the arena sounds consonant by default and dissonant
when crowded.

| Arena scalar (derived each frame) | Musical effect |
|---|---|
| Trail fill (total trail length / arena area) | Harmonic density, then dissonance: an empty arena is an open fifth, a carved-up one is a cluster |
| Alive count | Number of drone layers; **2 left → duel mode**: a pulse locks in |
| Nearest-threat distance to own rider | Tension filter sweep, sidechained under own-danger cues |
| Mean speed | Tempo of the pulse layer |
| Sim tick | **The clock**: 120 ticks/s = 120 BPM at 60 ticks per beat. Ambience events quantize to the sim beat, so the music is literally in time with the simulation |

Chord progressions come from a seed (the arena/room). Everyone in a room
hears the same harmony, though only through deterministic local derivation;
it is never replicated.

## 6. Budgets and invariants

- No sim, protocol or server change. `PROTOCOL_ID` is untouched.
- The audio thread never allocates or locks. `SoundFrame` is fixed-size
  (rider cap), and engines preallocate their voices.
- Missing frames hold the last state. Starvation fades to silence, never
  noise.
- CPU: well under 5% of one core native, inside the worklet's 2.7 ms quantum
  with headroom.
- Web size: the main bundle grows only by `AudioWorkletNode` bindings; the
  DSP wasm should stay under ~300 KB gzip. Measure as `building.md` does.
- `cargo fmt`/clippy clean, `tracing` only.

## 7. Phases

1. Presentation-event detector shared with fx (a small refactor; useful on
   its own).
2. `crates/audio` with the own-rider layer + cpal host + settings.
3. `lab --audio`: WAV + spectrogram; tune mappings on scenarios.
4. Opponent spatial layer and ambience.
5. Web worklet host.

## 8. Open questions for JH

1. **Web backend:** go with B (own worklet host, best web behavior, more
   plumbing), or A (`bevy_seedling`, native-first, web on the main thread),
   or native-only for now?
2. **Pure synthesis, or a few samples allowed** (e.g. a recorded derez layered
   on top)? Samples push toward A and add asset loading to the web build.
3. **Web size/compile budget:** is ~300 KB gzip for a DSP wasm and a slower
   client build (fundsp generics) acceptable, or should we hand-roll the DSP?
4. **Bots:** full voices like humans, or a quieter, simpler voice so they
   don't crowd the mix?
