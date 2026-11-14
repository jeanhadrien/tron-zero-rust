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
lists builds (`default_roster = true` for `bots::config::default_builds`,
plus `[[build]]` entries with personality, difficulty and knob overrides),
setups (rider count, `[setup.match]` rules, arena, optional `tier` pin) and a
seed count. Each setup plays every lineup of builds over every seed, with each
lineup rotated through all seats so spawn position does not favour a build.
Matches run in parallel (`rayon`); the report is identical whatever the thread
count.

```
cargo run --release -p tron-zero-lab -- tournament crates/lab/suites/ffa4.toml
cargo run --release -p tron-zero-lab -- tournament crates/lab/suites/rounds4.toml --json rounds4.json
cargo run --release -p tron-zero-lab -- tournament crates/lab/suites/ffa4.toml --no-noise --threads 4
```

`--no-noise` sets `noise = 0` on every build (how much of the play is
misjudgment, how much the planner itself); `--json PATH` also
writes the report as JSON. Columns are measured separately, with no combined
score:

- rounds (matches for a one-round setup) played; win rate (last-standing setups
  only; most rounds time out, see the line above the table);
- kills per minute of alive time; deaths by cause (wall, own trail, other
  trail); suicide share = (wall + own trail) / all deaths;
- median life in ticks (a survivor's life counts up to the round or match end,
  so a median at the time limit means most riders never died);
- idle share: alive ticks with no living enemy within `IDLE_RANGE`
  (`crates/lab/src/stats.rs`) and the nearest enemy not getting closer;
- rubber share: alive ticks in the rubber zone; mean speed multiplier;
- a kill matrix (row killed column); a mutual head-on counts a kill for both.

The last section, decision cost (mean and p99 microseconds per `Brain::think`),
is wall-clock and depends on the machine and the thread count: never compare
it across runs, and never assert on it.

Baseline suites live in `crates/lab/suites/` (1v1, FFA with 4 and 8 riders under
today's rules, and 4-rider last rider standing over 5 rounds); a release run of
all four takes a couple of minutes. To prove a bot change, run the same suite
before and after and compare the columns that the change targets.

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
