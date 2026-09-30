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
- `tron-zero-server` — bin, headless server + bots + manager HTTP.
- `tron-zero-client` — bin, rendering + UI + audio.

The WASM web client target (`wasm32-unknown-unknown`) is not set up yet — see `docs/building.md`.

## Gameplay smoke check

With a server and client running, use **A / Left Arrow** and **D / Right Arrow**
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

Rubber/speed HUD gauges remain a separate MVP milestone. Death feedback and
manual human respawn are implemented; multiplayer respawn, latency/rollback,
no-space retries and disconnect behavior still need the runtime smoke checks
above. `cargo check/clippy --tests` compile regression coverage only; they do
not execute tests or establish multiplayer runtime correctness.
