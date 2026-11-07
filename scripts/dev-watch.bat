@echo off
cd /d "%~dp0.."
where cargo >nul 2>nul || (echo cargo not on PATH. Open a new shell after rustup install. & pause & exit /b 1)
cargo watch --version >nul 2>nul || (echo Installing cargo-watch... & cargo install cargo-watch)
REM Bot debug visuals + server bot logs (grid overlay, rays, scores panel).
REM Spawned windows inherit this environment. Comment out to disable.
set TRON_BOT_DEBUG=1
REM Each watcher only restarts for its own crate or shared: a client-only edit
REM must not restart the server and drop the session.
start "tron-zero-server" cmd /k cargo watch -c -d 2 -w crates/shared -w crates/server -x "run -p tron-zero-server"
start "tron-zero-client" cmd /k cargo watch -c -d 2 -w crates/shared -w crates/client -x "run -p tron-zero-client"
echo Server + client watchers launched in separate windows.
