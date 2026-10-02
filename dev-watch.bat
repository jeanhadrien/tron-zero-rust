@echo off
cd /d "%~dp0"
where cargo >nul 2>nul || (echo cargo not on PATH. Open a new shell after rustup install. & pause & exit /b 1)
cargo watch --version >nul 2>nul || (echo Installing cargo-watch... & cargo install cargo-watch)
start "tron-zero-server" cmd /k cargo watch -c -d 2 -w crates -x "run -p tron-zero-server"
start "tron-zero-client" cmd /k cargo watch -c -d 2 -w crates -x "run -p tron-zero-client"
echo Server + client watchers launched in separate windows.
