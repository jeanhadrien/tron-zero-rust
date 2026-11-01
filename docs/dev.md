# Dev Build

Day-to-day development on Windows. Builds run natively on Windows for fast iteration and a visible game window.

## Prerequisites (one-time)

- **Windows Rust** — install via `https://win.rustup.rs/x86_64` (MSVC host). Pins to `stable` via `rust-toolchain.toml`.
- **MSVC C++ build tools** — Visual Studio 2022 Build Tools with the `VC.Tools.x86.x64` workload. rustup detects it automatically.
- **cargo-watch** — `cargo install cargo-watch` (hot-reload on file save).
- Open a **new** shell after install so `%USERPROFILE%\.cargo\bin` is on PATH. Verify: `where cargo` prints `C:\Users\<you>\.cargo\bin\cargo.exe`.

## Repo location

`C:\dev\tron-zero-rust` — the Windows filesystem. Editable from WSL at `/mnt/c/dev/tron-zero-rust`, but **run all cargo commands from Windows**. WSL's 9P bridge can write inconsistent mtimes that force cargo to recompile everything; staying on the Windows side for builds avoids that.

## The dev loop

From PowerShell / Windows Terminal, in `C:\dev\tron-zero-rust`:

```
cargo run -p tron-zero-client     # builds + runs the client (opens a window)
cargo run -p tron-zero-server     # builds + runs the headless server
cargo check -p tron-zero-client   # type-check only, no codegen — fastest feedback
cargo check --workspace --tests   # compile regression tests without executing them
cargo clippy --workspace --tests -- -D warnings
cargo clippy --workspace -- -D warnings
cargo fmt -- --check
```

### Hot-reload (auto restart on save)

In two separate terminals:

```
cargo watch -c -w crates -x "run -p tron-zero-server"
cargo watch -c -w crates -x "run -p tron-zero-client"
```

`-c` clears output on restart, `-w crates` ignores `target/`. Split panes in Windows Terminal with `Alt+Shift+D` / `Alt+Shift+-`.

## What to expect

- **First build:** several minutes — cargo downloads and compiles ~850 dependency crates (bevy + lightyear are large). One-time.
- **Edit your own code → `cargo run`:** seconds. Cargo recompiles only the crate you changed and re-links. Dependencies stay cached in `target/`.
- **No edit → `cargo run`:** ~1–2s. Cargo fingerprints, finds nothing stale, launches the existing `.exe`.
- **Big rebuilds only when:** you `cargo clean`, change a `Cargo.toml` dependency version, or toggle features. Rare.

If a build mysteriously rebuilds from scratch, you probably touched the repo from WSL. Run `git status` from Windows and rebuild from there.

## Targets

- `tron-zero-shared` — lib, the simulation core (no transport, no rendering).
- `tron-zero-bots` — lib, the bot brain (sense → decide over a world snapshot; no ECS systems, no networking).
- `tron-zero-server` — bin, headless server, bot roster/scheduling + manager HTTP.
- `tron-zero-client` — bin, rendering + UI + audio.
- `tron-zero-lab` — lib + bin, headless deterministic scenario runner (see below).

The WASM web client target (`wasm32-unknown-unknown`) is not set up yet — see `docs/building.md`.

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

A rider with `bot = "<personality>"` (and `elo`) is driven by the same brain
and decision schedule as the server's bots; leave out `at`/`heading` and it
gets a seeded safe spawn. `respawn_after = 240` mirrors the server's bot
respawn delay. `scenarios/bot_arena.toml` is the default five-bot roster:

```
cargo run -p tron-zero-lab -- crates/lab/scenarios/bot_arena.toml --seed 7
cargo run -p tron-zero-lab -- crates/lab/scenarios/bot_arena.toml --seed 7 --until 900 --png arena.png
```

Same seed, same game, so a bad bot death can be replayed, zoomed with
`--view` and re-run after a tweak. Bot tuning changes the `bot_arena` golden
trace: bless it once the change is intended.

Tests: `cargo test -p tron-zero-shared` (unit, property and rollback tests) and
`cargo test -p tron-zero-lab` (mechanics assertions + golden traces for every
scenario). After an intended behaviour change, regenerate goldens with
`LAB_BLESS=1 cargo test -p tron-zero-lab` and review the diff.

## Gameplay smoke check

### Menu and localhost connection

The client starts disconnected, with a menu over an empty grid. Start the
server separately, then click **Connect to Localhost** (or press Enter) to join
`127.0.0.1:5000`. There is no automatic connection or server process launch.
Only localhost is exposed for now; address entry, saved servers, and fetching
a server list from a master-server endpoint are deferred.

- The menu distinguishes connecting, synchronizing, and ready. A locally opened
  UDP socket alone is not readiness: the server must answer, timeline sync must
  complete, and the arena and controlled rider must arrive.
  Pending status identifies the clock, arena, or controlled rider/input buffer.
  Client logs record these transitions; the server logs its first valid heartbeat.
- Connecting/synchronizing offers **Cancel**. An attempt times out after
  10 seconds; an established session stops after 5 seconds without a heartbeat
  reply. Errors remain visible, and **Connect to Localhost** retries explicitly.
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
leave is lost). These runtime checks have not been executed by the agent.

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
  circle, and a **YOU DIED** overlay shows **Space / Enter** (including numpad
  Enter) to respawn. Holding the key does not repeat. Humans never auto-respawn.
  The client sends a request and waits; only the server resets the existing
  owned rider. Repeated requests while waiting do not allocate another rider.
- The server checks the connection owner, dead human status and life generation.
  If predicted death precedes authoritative death, the request can be rejected:
  wait for confirmation and press Space / Enter again. A rejected/no-space
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

Native Bevy UI shows remaining rubber (bar and percentage) at bottom left and
the current speed multiplier (for example `1.25x`) at bottom right. Rubber turns
amber at 50% and red at 20%; the bar follows depletion/regeneration immediately,
without smoothing. Speed reads the local predicted `SpeedMult`, including
collision slowdown; unlike the JS HUD, it does not freeze the pre-collision value.
There are no slide/collision indicators or gameplay control hints.

The meters are hidden while dead, outside a ready session, or while the menu is
open. Death uses a centered panel with the existing respawn instructions and
server-response feedback. All HUD entities persist across reconnects; they read
the current local predicted rider, not remote/confirmed copies.

Manual checks: resize the window, deplete/recover rubber, accelerate alongside a
wall, die and respawn, then disconnect/reconnect. Check that meters remain inside
the window, reset with the new life, and disappear behind the menu without
covering it. These visual checks require running the client.

Death feedback and manual human respawn are implemented; multiplayer respawn, latency/rollback,
no-space retries and disconnect behavior still need the runtime smoke checks
above. `cargo check/clippy --tests` compile regression coverage only; they do
not execute tests or establish multiplayer runtime correctness.
