# Release & Share

Status: updated 2026-11-02.

How a build reaches your friends. Dev loop lives in `docs/guides/dev.md`, manual
prod builds in `docs/guides/building.md`.

## Ship a client build

Releases are cut by release-please (already wired: push to `main` →
`release-please.yml` opens a version-bump PR → merge it → GitHub Release
created → `deploy.yml` builds and attaches the Windows exe).

1. Commit with a conventional message (`feat:`, `fix:`) and merge to `main`.
2. Merge the `release-please` PR it opens (one PR bumps shared/server/client
   together — linked versions).
3. Wait for the `deploy` workflow on that merge (~10–20 min, Bevy release
   build). It attaches `tron-zero-client-vX.Y.Z-windows-x86_64.zip` to the
   `client-vX.Y.Z` Release.
4. Share this link (always points at the newest Release):
   `https://github.com/jeanhadrien/tron-zero-rust/releases/latest`

Released clients point at `https://server-manager.tronzero.dev` by default
(baked in at build time via `TRONZERO_MANAGER_URL`; override with a repo
`MANAGER_URL` Actions variable, no code change). A `MANAGER_URL` env var at
runtime still wins, so dev builds keep pointing at localhost.

## Host a server for friends (your PC)

The game server speaks raw UDP on port 5000 — friends connect to *your*
machine, so it must be reachable from the internet:

1. Windows Firewall: allow inbound UDP on port 5000
   (`tron-zero-server`, private+public or scoped to taste).
2. Router: forward external UDP 5000 → your PC's LAN IP (DHCP reservation
   recommended so the target doesn't drift).
3. Run the server with your **public** IP advertised (not loopback):
   ```
   set MANAGER_URL=https://server-manager.tronzero.dev
   set ADVERTISED_HOST=<your-public-ip>
   set SERVER_NAME=JH dev
   set MAX_PLAYERS=10
   cargo run --release -p tron-zero-server
   ```
   The full variable list (`PORT`, `MAX_PLAYERS`, `ADVERTISED_PORT`, ...) is in
   `docs/guides/building.md`. The transport is not authenticated or encrypted yet, so
   treat a public server as a friends-only convenience.
4. Sanity: `curl https://server-manager.tronzero.dev/api/rooms` should list
   your room with the public host. Friends Refresh in-game and Connect.

CGNAT check: compare your router's WAN IP with a public "what is my IP"
lookup. If they differ, your ISP shares the address and home port-forwarding
cannot work — the fallback is the GCE VM in `deploy/gce/` (see its scripts; `udp:5000` and `udp:5001` firewall
rule, same env vars). On one network (LAN party), skip all of this and use
your LAN IP as `ADVERTISED_HOST`.
