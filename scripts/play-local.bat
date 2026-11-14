@echo off
REM One-click local test: server (with its bot roster) + client in separate
REM windows, the client auto-joining with a simulated ~20 ms round-trip ping.
REM
REM   scripts\play-local.bat            dev profile (deps are already fully optimised)
REM   scripts\play-local.bat release    --release build (slower first build, fastest game)
REM
REM Override before running, e.g.  set TRON_SIM_LATENCY_MS=80   (0 = no simulation)
REM Also honoured: TRON_SIM_JITTER_MS, TRON_SIM_LOSS_PCT (percent, per direction).
cd /d "%~dp0.."
where cargo >nul 2>nul || (echo cargo not on PATH. Open a new shell after rustup install. & pause & exit /b 1)

REM Dev client links Bevy as a DLL (crates/client "dev" feature) for faster rebuilds.
set "CARGO_FLAGS="
set "CLIENT_FLAGS=--features dev"
if /i "%~1"=="release" (set "CARGO_FLAGS=--release" & set "CLIENT_FLAGS=")

if not defined TRON_SIM_LATENCY_MS set "TRON_SIM_LATENCY_MS=20"
set "TRON_AUTOCONNECT=127.0.0.1:5000"
REM Game settings: 5 last-standing rounds. Set SERVER_CONFIG beforehand
REM for another file to play other rules (docs\specs\match-rules.md).
if not defined SERVER_CONFIG set "SERVER_CONFIG=crates\server\config\local.toml"
REM Windows inherit the variables above; the server ignores the client ones.
start "tron-zero-server" cmd /k cargo run %CARGO_FLAGS% -p tron-zero-server
REM The client keeps retrying until the server is up, so no sleep is needed.
start "tron-zero-client" cmd /k cargo run %CARGO_FLAGS% %CLIENT_FLAGS% -p tron-zero-client
echo Server and client launched (simulated RTT %TRON_SIM_LATENCY_MS% ms). Close both windows to stop.
