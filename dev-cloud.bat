@echo off
cd /d "%~dp0"
where cargo >nul 2>nul || (echo cargo not on PATH. Open a new shell after rustup install. & pause & exit /b 1)

set MANAGER_URL=https://server-manager.tronzero.dev/
set ADVERTISED_HOST=127.0.0.1
REM Deliberate: loopback host on a remote manager is refused by default.
set ALLOW_LOOPBACK_ANNOUNCE=1
if "%~1"=="" (set SERVER_NAME=JH dev) else (set SERVER_NAME=%~1)
REM Bot debug visuals + server bot logs. Enabled for local tuning;
REM comment out when advertising publicly to avoid extra replication.
set TRON_BOT_DEBUG=1

echo Starting server as "%SERVER_NAME%" against %MANAGER_URL%
start "tron-zero-server (cloud)" cmd /k "set MANAGER_URL=%MANAGER_URL% && set ADVERTISED_HOST=%ADVERTISED_HOST% && set ALLOW_LOOPBACK_ANNOUNCE=%ALLOW_LOOPBACK_ANNOUNCE% && set SERVER_NAME=%SERVER_NAME% && cargo run -p tron-zero-server"

echo Waiting for registration...
timeout /t 8 /nobreak >nul
echo Current rooms:
curl.exe -s %MANAGER_URL%api/rooms
echo.
echo Starting client against the same manager. Server browser, Refresh, Connect.
echo NOTE: this advertises 127.0.0.1:5000 on a PUBLIC list. Ctrl-C the server window when done.
start "tron-zero-client (cloud)" cmd /k "set MANAGER_URL=%MANAGER_URL% && cargo run -p tron-zero-client"
