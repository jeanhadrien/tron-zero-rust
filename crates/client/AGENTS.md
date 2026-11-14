# `client/` — desktop and browser client

Root rules: `../../AGENTS.md`. Browser decisions: `docs/specs/web.md`; build and serve: `web/README.md`.

## Purpose

What a player sees and touches: connection, input, prediction of the own
rider, rendering, UI. Desktop (UDP) and browser (wasm32, WebTransport) from the
same code. Lib + bin: logic stays in the lib so `nettest` can run the real app.

## Boundaries

- Depends on `shared/` only. Never on `server/`, `bots/` or `lab/`.
- The only place with rendering, `egui`, key bindings and persisted preferences.
- The client decides nothing about the game: it predicts its own rider with the
  shared sim and shows what the server replicates (match status, scores, bots).
- Target splits live in `platform.rs` only; other modules stay target-neutral.

## Map

- `connection` (lifecycle, readiness, autoconnect, quit), `input/` (keyboard queue, `TurnOutbox` redundant delivery), `look` (rider color/shape, sent as `SetLook`).
- `menu`, `settings`, `browser`, `config` (UI, bindings, room list, preferences), `chat`, `account` (manager account, ranked tickets, rating), `local_scan`.
- `theme` — design tokens; new UI uses them.
- `render/` — arena, trails, riders, `shape`, `smooth` (render pose), `fx`, `layers`, camera, HUD, `death`, `round` (round bar and banners), `scoreboard` (held Tab, end screen), bot `debug`.
- `sim` — latency simulator (`TRON_SIM_*`), the only `rand` user besides the first-launch look.

## Rules

- Own rider is `Predicted`; opponents and bots are step-interpolated. Do not add client-side guesses about other riders without a netcode decision (`NETWORK.md`).
- A one-shot player action is a message on a channel, never a `PlayerInput`.
- UI text states what the server said (for example the respawn refusal reason); it does not re-derive game state.

## Test

- `cargo check -p tron-zero-client` and clippy are the fast loop. Unit tests cover UI state (overlays, panels), not pixels.
- Do not launch the windowed client unless asked: an agent cannot play at game speed. What a client observes is tested through `nettest`.
- The wasm build is checked with `web/build.ps1` / `web/build.sh`. Known gap: the wasm check fails under sccache on Windows (command line too long); say so rather than skip silently.
