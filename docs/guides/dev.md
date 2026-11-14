# Dev Build

Status: updated 2026-11-08.

Day-to-day development on Windows. Builds run natively on Windows for fast iteration and a visible game window. Which commands to start for which setup (web client, prod lobby, GCP): `docs/guides/run.md`.

## Prerequisites (one-time)

- **Windows Rust** — install via `https://win.rustup.rs/x86_64` (MSVC host). Pins to `stable` via `rust-toolchain.toml`.
- **MSVC C++ build tools** — Visual Studio 2022 Build Tools with the `VC.Tools.x86.x64` workload. rustup detects it automatically.
- **bacon** and **cargo-nextest** — `cargo install --locked bacon cargo-nextest` (watch loop and test runner).
- Open a **new** shell after install so `%USERPROFILE%\.cargo\bin` is on PATH. Verify: `where cargo` prints `C:\Users\<you>\.cargo\bin\cargo.exe`.

## Repo location

`C:\dev\tron-zero-rust` — the Windows filesystem. Editable from WSL at `/mnt/c/dev/tron-zero-rust`, but **run all cargo commands from Windows**. WSL's 9P bridge can write inconsistent mtimes that force cargo to recompile everything; staying on the Windows side for builds avoids that.

## The dev loop

From PowerShell / Windows Terminal, in `C:\dev\tron-zero-rust`:

```
cargo run -p tron-zero-client --features dev   # builds + runs the client (opens a window)
cargo run -p tron-zero-server     # builds + runs the headless server
cargo check -p tron-zero-client   # type-check only, no codegen — fastest feedback
cargo clippy --workspace --all-targets -- -D warnings   # the lint gate
cargo fmt -- --check
cargo nextest run --workspace    # tests (config: .config/nextest.toml)
cargo test --workspace --doc     # doctests (nextest skips them)
```

The client's `dev` feature links Bevy as a DLL (`bevy/dynamic_linking`), so an
edit re-links only our code (about 2.7 s instead of 3.4 s for a client-only
change). Dev only: never with `--release` or for the web build. Building with
and without it keeps two builds of Bevy in `target/`.
Launch it through `cargo run`, which puts the Bevy and std DLLs on `PATH`.

If a running game locks `target\debug\*.exe` ("Access is denied"), `cargo check`,
`clippy` and `cargo test` for library crates still work; only linking the
binaries fails.

### Watch loop (bacon)

`bacon` re-runs a job on save and shows the errors in place. Jobs live in
`bacon.toml`:

```
bacon                 # clippy over the workspace (the lint gate), the default
bacon test            # cargo nextest run -p tron-zero-shared
bacon server          # restart the server on each change
bacon client          # restart the client (dev feature) on each change
```

Inside bacon, `?` lists the keys (`c` clippy, `t` test, `q` quit);
`bacon test -- -p tron-zero-lab` targets another package. Set
`TRON_BOT_DEBUG=1` first for the bot overlay. Split panes in Windows Terminal
with `Alt+Shift+D` / `Alt+Shift+-`.

Typical setup: one pane on `bacon` (a clean pane means CI's lint gate passes),
and `scripts\play-local.bat` when you want to play. Keep a single bacon
running: every cargo command shares `target/`, so a second one waits on
"Blocking waiting for file lock". `bacon client` restarts the game window on
every save (including `shared`), which suits visual tweaks, not long sessions.

## What to expect

- **First build:** several minutes — cargo downloads and compiles ~850 dependency crates (bevy + lightyear are large). One-time.
- **Edit your own code → `cargo run`:** seconds. Cargo recompiles only the crate you changed and re-links. Dependencies stay cached in `target/`.
- **No edit → `cargo run`:** ~1–2s. Cargo fingerprints, finds nothing stale, launches the existing `.exe`.
- **Big rebuilds only when:** you `cargo clean`, change a `Cargo.toml` dependency version, or toggle features. Rare.

If a build mysteriously rebuilds from scratch, you probably touched the repo from WSL. Run `git status` from Windows and rebuild from there.

## Targets

- `tron-zero-shared` — lib, the simulation core (no transport, no rendering).
- `tron-zero-bots` — lib, the bot brain (plan maneuvers over a world snapshot, check them with the real physics; no ECS systems, no networking).
- `tron-zero-server` — lib + bin, headless server: netcode wiring in the lib (`add_core`), bot roster/scheduling and manager HTTP in the bin.
- `tron-zero-client` — lib + bin, rendering + UI (egui); the lib holds connection, input and prediction so tests can drive them without a window.
- `tron-zero-lab` — lib + bin, headless deterministic scenario runner (see below).
- `tron-zero-nettest` — lib + tests, real server and clients over loopback UDP (see below).
- `server-manager/` — Bun/TypeScript lobby service (see its README).

The browser client (`wasm32-unknown-unknown`) is built with `web/build.ps1` / `web/build.sh`; see `web/README.md` and `docs/guides/building.md`.

## One-click local test (`scripts\play-local.bat`, `scripts/play-local.sh`)

```
scripts\play-local.bat            # dev profile
scripts\play-local.bat release    # --release (longer first build, fastest game)
```

Opens two windows: the server (with its bot roster) and the client, which
joins `127.0.0.1:5000` by itself and plays with a **simulated 20 ms round-trip
ping**. The dev profile already builds dependencies at `opt-level = 3` and our
crates at 1 (see `Cargo.toml`), so it is playable and shares `target/` with
`bacon`; use `release` only if the frame rate disappoints.

Environment variables (all opt-in, read by the client; `TRON_BOT_DEBUG=1` is read by both server and client and enables the bot overlay):

| Variable | Effect |
|----------|--------|
| `TRON_SIM_LATENCY_MS` | Simulated **round-trip** latency. The bat defaults it to 20; `0` or unset disables. |
| `TRON_SIM_JITTER_MS` | Uniform ± jitter applied to each direction. |
| `TRON_SIM_LOSS_PCT` | Packet loss percentage applied to each direction. |
| `TRON_AUTOCONNECT` | `host:port` to join at startup; retries every 1.5 s until the server is up. Any menu action by hand (Cancel, Disconnect, Quit) turns it off, as does reaching play once. |
| `TRON_CONFIG_FILE` | Use this settings file instead of the platform one. |
| `MANAGER_URL` | Lobby base URL for the server browser (see Settings file for precedence). |

Server environment variables (read once at start; `MAX_PLAYERS`, `PORT`, `WEB_PORT` and the lobby ones are in `docs/guides/building.md`). The game mode is described in `docs/specs/rounds-mode.md`; `scripts/play-local.*` sets `GAME_MODE=rounds`:

| Variable | Effect |
|----------|--------|
| `GAME_MODE` | `ffa` (default: free-for-all, manual respawns) or `rounds` (last rider standing per round). |
| `ROUNDS` | Rounds per match in rounds mode (default 5). |
| `ROUND_SECS` | Round time limit in seconds (default 90; `0` = no limit). |
| `INTERMISSION_SECS` | Break between rounds, and before round 1 (default 3). |
| `FINAL_INTERMISSION_SECS` | Break showing the final standings (default 8). |
| `BOT_ROSTER` | Path of a roster file (see § Ladder) to seat instead of the built-in one, `crates/bots/rosters/default.toml` (embedded at build time: five difficulty rungs and the artist Escher). One bot per build; logged at `info!` at start; an invalid file stops the server. |

**How the simulation works.** Lightyear's link conditioner only delays what a
peer *receives*, so a client-only conditioner would shape just the server ->
client direction. The client therefore splits the requested RTT: half goes to
the conditioner on its receive side, half to a small uplink queue
(`crates/client/src/sim.rs`) that holds outgoing packets before they reach the
socket. Loopback adds ~0 ms, so the RTT you experience is the requested one.
`crates/nettest/tests/session.rs` checks this against Lightyear's own RTT
estimate (100 ms requested -> ~100 ms measured). Jitter and loss apply to each
direction independently; the uplink keeps packet order (jitter stretches, never
reorders). When active the client logs a `NETWORK SIMULATION ACTIVE` warning
and the in-game badge (top right) shows an amber `sim 20ms` next to the
measured `ping`.

## Settings file

The client persists the player name, turn key bindings, rider look (color,
shape), theme, effects toggle, saved accounts and an optional `manager_url`
to `client.json` in the platform config dir under `tron-zero\`
(`%APPDATA%\tron-zero\client.json` on Windows; `TRON_CONFIG_FILE` overrides).
Preferences are written debounced and atomically after a change. Reading or writing can fail without
consequence: a missing, corrupt or illegal file falls back to defaults and a
failed save is only logged. Manager URL precedence: `MANAGER_URL` env, file,
build-time `TRONZERO_MANAGER_URL`, `http://localhost:3001`.

## Lobby (server-manager)

The server browser needs the Node/Bun lobby service in `server-manager/`
(see its README for the API). To run the full loop locally:

```
cd server-manager
bun install          # first time only
bun run dev          # listens on :3001 (PORT to override)
```

Then start the Rust server (`MANAGER_URL` defaults to `http://localhost:3001`)
and open the client's Rooms screen. The server registers, heartbeats every 2 s
and re-registers on a 404; without the manager it retries every 5 s and the
client still lists any server it detects on this machine. Server env vars (`MANAGER_URL`,
`ADVERTISED_HOST`, `ALLOW_LOOPBACK_ANNOUNCE`, ...) are in `docs/guides/building.md`;
the API and the announce protocol are in `server-manager/README.md`.

`scripts/dev-cloud.bat [name]` starts a local server that announces itself to the
Cloud Run manager (with `ALLOW_LOOPBACK_ANNOUNCE=1` and `ADVERTISED_HOST=127.0.0.1`,
so it puts a loopback address on a **public** list; close the server window
when done) and a client pointed at the same manager.

## Netcode harness (nettest)

`crates/nettest` runs the real server app and real client app(s) in one
process over loopback UDP, with Lightyear's link conditioner (latency, jitter,
loss) and the client's own `TRON_SIM_*` uplink queue. Tests script key presses
and assert on the client/server traces.

```
cargo nextest run -p tron-zero-nettest                                    # serialized, wall-clock
cargo test -p tron-zero-nettest --test measure -- --ignored --nocapture   # arrival-margin table
```

It uses wall-clock timing and an unseeded conditioner, so a failure should be
re-run alone before treating it as real. Under nextest each test is its own
process, so `.config/nextest.toml` serializes the package with a test group
and retries a failure once (a pass on retry is reported as `FLAKY`). Results and the contract it checks are
in `docs/specs/netcode-design.md`.

## Lab (headless scenarios)

The sim is deterministic and tick-stepped, so situations can be scripted and
replayed exactly instead of played at 120 Hz. A scenario places riders and
schedules their actions per tick (format documented in
`crates/lab/src/scenario.rs`):

```toml
until = 300
walls = [[-1000, 6, 1100, 6]]   # optional extra segments

[[rider]]
name = "a"
at = [-900, 0]
heading = "+x"                  # y points up, as in the client
script = ["120 left", "200 right", "260 respawn"]
```

```
cargo run -p tron-zero-lab -- crates/lab/scenarios/grind.toml --sample 60 --png grind.png
cargo run -p tron-zero-lab -- my.toml --until 150 --view 1190,0,60 --png zoom.png
cargo run -p tron-zero-lab -- my.toml --frames frames --every 30
```

Output is a JSONL trace (`turn`, `enter/leave_rubber_zone`, `slide_start/end`,
`death` with its cause, `respawn`, optional `sample`) plus a `#` summary, and
PNGs on request. Respawns mirror the server's safe-spawn search. The summary
also counts kills and groups deaths by cause.

### Bot arenas

A rider with `bot = "<personality>"` (and `difficulty`, plus optional per-knob overrides in `[rider.knobs]`) is driven by the same brain
and decision schedule as the server's bots; leave out `at`/`heading` and it
gets a seeded safe spawn. `respawn_after = 240` mirrors the server's bot
respawn delay. `scenarios/bot_duel.toml` is a weak bot against a strong one (the weak one dies and respawns):

```
cargo run -p tron-zero-lab -- crates/lab/scenarios/bot_duel.toml --seed 7
cargo run -p tron-zero-lab -- crates/lab/scenarios/bot_duel.toml --seed 7 --until 900 --png arena.png
```

Each brain seeds its misjudgments (`noise`: it reads a wall as farther or a pocket
as larger than it is, it never acts at random) from its own seed: the lab
derives it from the scenario seed, the rider and the life, the server draws a
random one per bot. Same seed, same game, so a bad bot death can be replayed,
zoomed with `--view` and re-run after a tweak. Bot tuning changes the
`bot_duel` and `rounds` golden traces: bless them once the change is intended.

The brain plans one or two turns at a time, scores each candidate by a weighted
sum (personality = weights, `bots::objective`; difficulty = reaction, horizon,
variety, noise, `bots::config`), and checks the best ones with the real physics
before riding them. `crates/lab/puzzles/` holds hard spots (closed room, dead
end, narrow gap) with one bot each; `tests/gates.rs` requires strong builds to
solve them and a small tournament to stay under a self-kill rate.

Fighting: the planner adds three terms, `pressure` (squeeze or seal the
target's territory, or cut it off), `control` (own share of the race over
riding straight) and `engage` (head toward the nearest opponent), all gated by
a safety factor and by contact distance. `Personality::Passive` ("passive" in a
scenario's `bot_build`) zeroes them: use it as the reference opponent.
`puzzles/intercept.toml` has a scripted straight rider the duelist must cut
off. `puzzles/hunt.toml` starts the bot beside a straight rider's trail: it must
grind to speed, pass the rider and cut it off (only the duelist at D4000 does).
`suites/fight.toml` runs each personality (all at difficulty 2000) and Passive in
1v1 rounds.

Speed, rubber and heat (phase 4): `grind` is the speed a line earns riding
beside trails and walls (`bots::grind`, the sim's own constants; the bot's own
trail is left out on purpose, bots that coil along it box themselves in), and
capped at 1.5x so the bot hunts once it is fast. `lanes` adds candidates aimed
beside the obstacle ahead and to each side. `rubber` prices the burn a line
causes against what it faces (late turns inside the rubber zone) as a share of
the rubber left; personalities differ in how dear it is. A target that burned
its rubber is prey. A bot told it plays a round (`Brain::reborn_for_round`)
heats up with the round's age (`Weights::heated`): fight terms rise at once,
room, margin and rubber's price fall later (`WEAR_AT`), so rounds end. Risk
below the calm window (`Weights::risk`) is only taken for a kill chance: a
`bold` line must reach `BOLD_PRESSURE` (fading with the heat) or it is dropped
from the fight. Without that gate the fighters died of their own risk window and
lost to Passive; with it they win more rounds than Passive, but fewer rounds end
(wear is what ends them, and it hits every rider alike). Nothing in ffa (respawn)
heats up. Measured limit: a competent evader always has an open side, so kills
need it boxed in; most deaths in rounds stay self-kills.
The grids assume the default 2000-unit arena (`bots::space`).

`tier = "full"` (or `t1`, `t2`) at the top of a scenario pins the bots' level
of detail; without it the tier follows the number of living riders, as on the
server.

### Rounds (last rider standing)

A `[match]` block (`shared::MatchRules`: respawn policy, end condition, number
of rounds) turns a scenario into a match. It replaces the riders'
`respawn_after`. With `respawn = "never"` and `last_standing = true`, nobody
respawns during a round and the round ends when at most one rider is alive (or
at `time_limit`). The winner is the last rider alive: a time-out with several
alive, or the last riders dying on the same tick, has no winner. Then every
rider is reset and placed again with the safe-spawn search and a new seed, and
the next round starts. The trace has `round_start` and `round_end` (with the
winner and the reason) events; `round_end` carries the round's last tick. The
run stops when the match is over, `until` stays the hard cap (set it to at
least `rounds * time_limit`).

```
cargo run -p tron-zero-lab -- crates/lab/scenarios/rounds.toml --seed 7
```

### Tournaments (comparing bot builds)

`lab tournament <suite.toml>` plays bot builds against each other and prints
one markdown table per setup. A suite (format in `crates/lab/src/tournament.rs`)
lists builds (`roster = "builtin"` for the bots the server seats, or a roster file;
plus `[[build]]` entries with personality, difficulty and knob overrides),
setups (rider count, `[setup.match]` rules, arena, optional `tier` pin) and a
seed count. Each setup plays every lineup of builds over every seed, with each
lineup rotated through all seats so spawn position does not favour a build.
Matches run in parallel (`rayon`) on half the machine's cores unless `--threads N`
says otherwise (tournament, tune and ladder alike); the report is identical
whatever the thread count.

```
cargo run --release -p tron-zero-lab -- tournament crates/lab/suites/rounds4.toml --quick
cargo run --release -p tron-zero-lab -- tournament crates/lab/suites/ffa4.toml
cargo run --release -p tron-zero-lab -- tournament crates/lab/suites/rounds4.toml --json rounds4.json
cargo run --release -p tron-zero-lab -- tournament crates/lab/suites/ffa4.toml --no-noise --threads 4
```

`--quick` cuts the seed count to the suite's `quick_seeds` (default: an eighth of
`seeds`): the normal way to measure, well under a minute per baseline suite on 4
threads. The full counts are for a rare final check. `--no-noise` sets `noise = 0` on every build (how much of the play is
misjudgment, how much the planner itself); `--json PATH` also
writes the report as JSON. Columns are measured separately, with no combined
score:

- rounds (matches for a one-round setup) played; win rate (last-standing setups
  only; most rounds time out, see the line above the table);
- kills per minute of alive time, direct and forced kills (see below); deaths by
  cause (wall, own trail, other trail); self-kill share = unforced wall and
  own-trail deaths / all deaths;
- median life in ticks (a survivor's life counts up to the round or match end,
  so a median at the time limit means most riders never died);
- idle share: alive ticks with no living enemy within `IDLE_RANGE`
  (`crates/lab/src/stats.rs`) and the nearest enemy not getting closer;
- rubber share: alive ticks in the rubber zone; mean speed multiplier;
- pattern: share of the ridden path that is a repeated motif, see
  "Pattern score" below;
- a kill matrix (row killed column) of direct and forced kills; a mutual head-on
  counts a kill for both.

Forced kills (`crates/lab/src/forced.rs`, lab only: the sim and the ranked rating
still credit the obstacle faced). A wall or own-trail death is forced when, just
before it, the rider's reachable room (20-unit flood fill) was below a pocket
(300 cells) and the trail an opponent laid in the last 240 ticks closed it: without
it the room is at least 300 cells and twice as large. The opponent giving back
the most room gets the kill. A rider coiling alone has no opponent trail in its
pocket and stays a self-kill; `lab <scenario>` prints each such death and who
forced it, to check against `--frames`.

A suite build is `name`, `bot` (personality, any case), optional `difficulty`
(the authoring preset its knobs start from), `[build.knobs]` (any knob of
`bots::config::BotKnobs`) and `[build.weights]` (any weight of
`bots::objective::Weights`). Knobs and weights are matched by name against the
bots crate's serde form, so a weight added or renamed there works in suites
with no lab change; an unknown name is an error. `roster = "file.toml"` (or
`"builtin"`, the roster the server seats) adds a
roster file (see "Ladder").

The last section, decision cost (mean and p99 microseconds per `Brain::think`),
is wall-clock and depends on the machine and the thread count: never compare
it across runs, and never assert on it.

Baseline suites live in `crates/lab/suites/` (1v1, FFA with 4 and 8 riders under
today's rules, and 4-rider last rider standing over 5 rounds); a release run of
all of them takes a few minutes, `--quick` about 30 seconds each (`rounds4` is
the slowest). To prove a bot change, run the same suite
before and after and compare the columns that the change targets.

### Pattern score (the `pattern` column)

JH's idea of fun includes bots that draw repeatable shapes with their trails.
`crates/lab/src/pattern.rs` scores it from the turn events of each life:

- A path is a sequence of legs: the turn side (left or right) and the length
  ridden before the next turn. A life's first turn only sets the heading and
  the leg after the last turn is unfinished, so neither is a leg.
- A stretch of legs is patterned when, for a period `p` of 1 to 4 legs, each
  leg has the side of the leg `p` before it and the length difference to that
  leg stays the same from one leg to the next (within 15 units + 10%). Zero
  difference is a repeated motif (staircase `L R L R`, zigzag, square wave
  `L R R L`); a constant nonzero difference is a growing one (square spiral:
  always left, legs +60 each). It must run for two periods (three if it
  grows, since two legs always form a progression).
- Score = length of the patterned legs / length of the whole path ridden
  (alive ticks x speed, so a straight life, a single turn or a random walk
  score near 0 and a clean staircase or spiral near 1). Summed over lives and
  rounds, not averaged per life.

It is a proxy. It does not see shapes drawn by more than 4 distinct legs,
mirrored or rotated repeats of a bigger figure, curves of many small legs
that vary by more than the tolerance, or whether the pattern is worth looking
at; U-turns count as two ordinary turns with a short leg. Wandering bots land
around 0.05 to 0.1 by chance; compare builds, not absolute values.

### Artists (the `style` term)

An artist rides a figure instead of a fight: legs of one length (`stride`, in
units) and a repeating cycle of turn sides (`motif`: 0 staircase, 1 square
wave), `crates/bots/src/style.rs`. `Personality::Artist` has `style` on and the
fight and grind weights at zero (it never hunts and holds a constant speed, so
legs stay equal); fighters keep `style = 0`. The planner adds the turns that
continue the figure to its candidates and rates every line by how well it
continues the figure (a turn when the leg reaches the stride, on the side the
cycle asks for, rates 1; the mirrored side costs a little, which lets it turn
away from a wall); `style` only counts for a line with room, so survival wins.
Use it in a scenario or suite with `bot = "artist"` and `[rider.weights]` /
`[build.weights]` `motif = 1.0`, `stride = 200.0`. The game's artist is
`Escher (Artist)` in `crates/bots/rosters/default.toml`, outside the ladder.
It draws about 50-60% `pattern` in a crowd against 5-25% for the fighters, with
no wall deaths. A meander (R R L L) was tried and dropped: it jams against walls.
`tests/gates.rs` gates the pattern score and the artist's own-fault deaths.
To look at it: `lab <scenario with an artist> --png out.png --until 1000`.

### Tune (sweeping a build's knobs and weights)

`lab tune <tune.toml>` sweeps the parameters of one build for a game setup. The
file (format in `crates/lab/src/tune.rs`, example in
`crates/lab/tuning/rounds4.tune.toml`) names the base build, the parameters to
sweep with their ranges (any knob or weight, by bare name), the setups, the
reference pool, the seeds and the fitness: a weighted sum of tournament
metrics (`win_rate`, `kills_per_min`, `direct_deaths_per_round`, `forced_deaths_per_round`,
`self_kill_share`, `own_deaths_per_round`,
`median_life_ticks`, `idle_share`, `rubber_share`, `mean_speed`, `pattern`),
penalties being negative weights. The candidate always plays; its seat rotates
and the other seats take a window of the pool that moves with the seed.

```
cargo run --release -p tron-zero-lab -- tune crates/lab/tuning/rounds4.tune.toml
cargo run --release -p tron-zero-lab -- tune crates/lab/tuning/rounds4.tune.toml --out best.toml --json sweep.json
```

- Each parameter moves over its range (`[sweep] steps` values), the others at
  the base, and every metric is printed along it. A parameter whose every
  metric stays identical is flagged DEAD (a knob that does nothing in this
  setup, or whose range is too small).
- `--out` writes the base build with the single parameter value that scored
  best as a `[[build]]` entry (paste it in a suite or a roster file); `--json`
  writes the full report.
- Fitness is deterministic for a build and a seed set, and the report is the
  same on any `--threads`. Cost is about parameters x steps x (seeds x riders)
  matches of `rounds` rounds each. Few seeds make a noisy fitness: read a
  sweep as a hint, not as a result. Rounds that time out count in `idle_share`
  and `win_rate` like any other.

### Ladder (generated difficulty rungs)

`lab ladder <ladder.toml>` (format in `crates/lab/src/ladder.rs`, example in
`crates/lab/tuning/duel.ladder.toml`) degrades one build along its budget
knobs, from itself to the weakest budget (`degrade_to`, an authoring preset;
`[degrade]` overrides single knobs), in `candidates` steps. Each step is
measured against the reference pool (`strength` = any tournament metric,
default `win_rate`), `rungs` of them are picked evenly spaced in measured
strength, and every pair of rungs plays head to head (both seats, `check_seeds`
seeds, the first setup's rules with two riders) to check that each higher rung
wins more rounds than each lower one. The report says whether the ladder is
monotonic; the roster is written either way.

```
cargo run --release -p tron-zero-lab -- ladder crates/lab/tuning/duel.ladder.toml --out ladder.toml
```

`strength = "duels"` (what the game's ladder uses) needs no pool: every
candidate plays every other head to head in 1v1 rounds and its strength is its
share of decided rounds. A pool metric such as `win_rate` against the presets
barely separates builds (most rounds end on the time limit, and the budget
knobs move survival more than winning), `-name` flips a metric where lower is
better. `duel.ladder.toml` also degrades past the presets (`[degrade]`), because
rungs close to each other do not separate in a quick check.

The output is a roster file (`bots::roster`: `[[build]]` entries with
personality, every knob and every weight). The lab reads it with
`roster = "ladder.toml"` in a suite or a pool.

The server's roster is `crates/bots/rosters/default.toml`: the five rungs of the
last `duel.ladder.toml` run, weakest first (`Wisp (Novice)`, `Drifter (Easy)`,
`Warden (Medium)`, `Raptor (Hard)`, `Apex (Expert)`; names carry a difficulty
label and are at most 16 characters), renamed by hand. `bots::roster::builtin`
embeds it, so the binary needs no file; `BOT_ROSTER=path` loads another one.
`tests/roster.rs` checks that each rung beats the one below. After a brain
change that moves strengths, regenerate the rungs and rename them.
The lab measures builds from roster files (`roster = "builtin"` is this one);
`difficulty = N` (`bots::config::Difficulty`) stays only as the shorthand of
scenarios, puzzles and suites for a budget curve, and as the weakest end
(`degrade_to`) of a ladder. No bot is defined twice.

Tests: `cargo nextest run -p tron-zero-shared` (unit, property and rollback tests) and
`cargo nextest run -p tron-zero-lab` (mechanics assertions + golden traces for every
scenario; a scenario without a golden fails). After an intended behaviour change, regenerate goldens with
`LAB_BLESS=1 cargo nextest run -p tron-zero-lab` and review the diff.

## Gameplay smoke check

### Menu and localhost connection

The client starts disconnected, with a menu over an empty grid (unless
`TRON_AUTOCONNECT` is set, see above). Type a **Name** (16 characters max,
saved), start the server separately, then open **Server browser** and click
**Connect** on the localhost row (always listed first). It appears once a
server answers a probe on `127.0.0.1` ports 5000-5003; the menu re-probes
about every 1.5 s while it is open and offline (never during play). A second
local server needs its own `PORT` and `WEB_PORT` (e.g.
`PORT=5002 WEB_PORT=5012`); 5001 is the default WebTransport port and never
answers. The browser build cannot probe UDP, so it lists localhost
only on a local dev host with `?TRON_CERT_DIGEST=` (see `docs/guides/run.md`). Full rooms show
a disabled **Full** button. **Quit game** leaves the server first (so the rider
is removed at once) and then exits; the window close button does the same.

The first message of every session is a hello carrying the protocol id
(`PROTOCOL_ID` in `protocol.rs`: `PROTOCOL_VERSION`, which you bump by hand when
the wire format or gameplay logic changes, mixed with a fingerprint of every
constant in `constants.rs`, which updates itself; a test fails if a new constant
is not listed in `SIM_CONSTANTS`) and the requested name. The server only spawns
the rider (and starts replicating to the link) after accepting it: a version
mismatch or a full server (`MAX_PLAYERS` humans; bots do not count) is answered
with a reason the menu shows, and the link is closed about a second later, once
the client has said goodbye or the rejection had time to be retransmitted. The
server trims the name, strips control and invisible format characters (bidi
overrides, zero-width), caps it at 16 characters, falls back to `Player` and
appends ` 2`, ` 3`... if another player or bot already uses it (compared
ignoring case and spaces).

- The menu distinguishes connecting, synchronizing, and ready. A locally opened
  UDP socket alone is not readiness: the server must answer, timeline sync must
  complete, and the arena and controlled rider must arrive.
  Pending status identifies the clock, arena, or controlled rider/input buffer.
  Client logs record these transitions; the server logs its first valid heartbeat.
- Connecting/synchronizing offers **Cancel**. An attempt times out after
  10 seconds; an established session stops after 5 seconds without a heartbeat
  reply. Errors remain visible, and connecting again is an explicit click.
- **Escape** toggles the translucent overlay during play. **Resume** or Enter
  closes it. The live session and camera continue behind it: your rider keeps
  moving and can die. This is not a pause or invulnerability mode.
- While the menu is open, turning and respawn keys are captured and queued turns
  are cleared. The frame that closes the menu is also captured, so Enter cannot
  simultaneously resume and respawn. The death prompt returns after Resume.
- **Disconnect** leaves the session and returns to the menu. Lightyear removes
  replicated entities before reconnection becomes available. The camera, menu,
  and death overlay are reused; input and respawn UI state are reset.
- A leave message is flushed before closing the client socket. Delivery is
  best-effort at shutdown; the server's 5-second heartbeat lease removes
  abandoned riders/links if the leave is lost or the client exits abruptly.

Manual checks: connect with no server, cancel and retry, then start the server
and connect. Open/close the overlay during rapid turns and while dead; check
that no menu keys become turns or respawn requests. Disconnect/reconnect several
times and confirm one controlled rider, one camera, and no old trails. Stop the
server during play and verify a timeout with a retry option. With a second
client, verify the departing rider disappears (within the lease timeout if the
leave is lost). These runtime checks have not been run by a human yet (see `NETWORK.md`).

### Riding and respawning

After connecting, use **A / Left Arrow** and **D / Right Arrow**
to turn left and right relative to the rider's heading.

Each physical keypress queues one relative turn; holding a key does not repeat.
Multiple presses in one frame (including A + Left Arrow) keep their OS event
order and execute on consecutive 120 Hz ticks, not all in one tick. The queue
has no fixed burst limit. Keyboard collection precedes simulation; fresh input
is consumed only when the network input buffer is ready and never during
rollback. Local prediction adds no configured input-delay ticks.

- On death, the trail disappears, the stationary rider becomes a red crossed-out
  circle, and a **YOU DIED** overlay shows **Space** to respawn (Enter opens the
  chat box and never respawns). Holding the key does not repeat. Humans never auto-respawn.
  The client sends a request and waits; only the server resets the existing
  owned rider. Repeated requests while waiting do not allocate another rider.
- The server checks the connection owner, dead human status and life generation.
  If predicted death precedes authoritative death, the request can be rejected:
  wait for confirmation and press Space again. A rejected/no-space
  request displays a retry prompt; it does not move the rider or restore rubber.
- Initial human spawn and manual respawn search the actual arena walls, live
  trails and rider positions. Candidates require a 100-unit boundary margin
  and an 80-unit clearance corridor extending 360 units in the chosen heading.
  If no safe candidate is found, the human stays dead and may manually retry.
  This is clearance from **current** geometry, not immunity from moving riders
  or trails grown later. Bots retain their existing spawn/lifecycle behavior.
- Queue several turns just before dying; press turn keys while dead, then
  respawn. No pre-death/dead-state turns should execute in the new life. Fresh
  turns after respawn still execute one per tick in OS event order. Disconnect
  and reconnect: there must be no carried-over turn queue, extra camera, overlay,
  or controlled rider.
- Approach an arena wall without turning. The rider should slow before contact,
  consume rubber, then die and lose its trail rather than immediately die at the
  boundary or cross it.
- Turn away before rubber runs out. Movement should resume and rubber should
  regenerate. Passing close alongside a wall or trail should accelerate the
  rider; returning to open space should gradually restore base speed.
- Cross an opponent's trail, then loop back toward your own older trail. Both
  should engage the same rubber response, including the opponent's growing
  segment. A normal turn must not count as crashing into your own new corner.
- Survive long enough to reach the trail cap. The oldest end should shorten
  continuously, including on a long straight; it must not disappear wholesale.
- Connect a second client and compare trail corners, shrinking tails, and death
  cleanup on both views. Check rapid consecutive turns for duplicate trails or
  visible disagreement after server corrections.

### Player HUD

The egui HUD (`render/hud.rs`) shows a frameless rubber meter with its
percentage at bottom left and a ping badge at top right (amber `sim Nms` when
the latency simulator is on). There is no speed readout, slide/collision
indicator or control hint. The meter color follows the theme and the rubber
fraction; the bar follows depletion/regeneration immediately, without
smoothing, and reads the local predicted rider, not remote/confirmed copies.

The meter is hidden while dead, outside a ready session, or while the menu is
open. Death uses a centered panel with the respawn instructions and
server-response feedback.

Manual checks: resize the window, deplete/recover rubber, accelerate alongside a
wall, die and respawn, then disconnect/reconnect. Check that meters remain inside
the window, reset with the new life, and disappear behind the menu without
covering it. These visual checks require running the client.

Death feedback and manual human respawn are implemented; multiplayer respawn, latency/rollback,
no-space retries and disconnect behavior still need the runtime smoke checks
above. Unit tests and nettest cover the logic, not the windowed UI.
