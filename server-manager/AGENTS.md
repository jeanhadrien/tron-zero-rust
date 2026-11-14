# `server-manager/` — lobby service (Bun + TypeScript)

Root rules: `../AGENTS.md`. API, env vars and deployment: `README.md`. Rating design: `docs/specs/matchmaking-elo.md`.

## Purpose

The lobby: game servers register and heartbeat, clients list rooms. Also
anonymous accounts, join tickets, Elo ratings, leaderboard and placement.

## Boundaries

- Separate from the Rust workspace: it shares no code with it, only the HTTP
  contract. A change to that contract is made on both sides in the same change
  (`crates/server/src/announce.rs`, `ranked.rs`; `crates/client/src/browser.rs`, `account.rs`).
- It knows nothing about the simulation. It rates the elimination events the
  game server reports; it never re-derives who killed whom.
- Rooms are in memory. Accounts, ratings, keys and tickets go through the store
  interface (`src/store/`: memory or Firestore); no direct Firestore calls elsewhere.

## Map

- `main.ts`, `index.ts` — Express app. `room.ts`, `roomStore.ts` — registry.
- `accounts.ts`, `identity.ts`, `tickets.ts`, `ranked.ts`, `results.ts`, `placement.ts`.
- `rating/` — Elo (`elo.ts`, `pool.ts`, `replay.ts`); tuning constants in `rating/config.ts`.
- `rateLimit.ts`, `logger.ts`, `scripts/` (ops and smoke scripts).

## Dev loop

- `bun run dev` (watch mode, `:3001`), `bun run typecheck` before committing.
- A bot's rating id comes from its build (name and fingerprint): a retuned or
  renamed bot is a new rated identity.
