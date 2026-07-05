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
