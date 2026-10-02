@echo off
cd /d "%~dp0"
where cargo >nul 2>nul || (echo cargo not on PATH. Open a new shell after rustup install. & pause & exit /b 1)

set MANAGER_URL=https://server-manager-335435262909.europe-west1.run.app/
set ADVERTISED_HOST=127.0.0.1
if "%~1"=="" (set SERVER_NAME=JH dev) else (set SERVER_NAME=%~1)

echo Starting server as "%SERVER_NAME%" against %MANAGER_URL%
start "tron-zero-server (cloud)" cmd /k "set MANAGER_URL=%MANAGER_URL% && set ADVERTISED_HOST=%ADVERTISED_HOST% && set SERVER_NAME=%SERVER_NAME% && cargo run -p tron-zero-server"

echo Waiting for registration...
timeout /t 8 /nobreak >nul
echo Current rooms:
curl.exe -s %MANAGER_URL%api/rooms
echo.
echo Starting client against the same manager. Browse Servers, Refresh, Connect.
echo NOTE: this advertises 127.0.0.1:5000 on a PUBLIC list. Ctrl-C the server window when done.
start "tron-zero-client (cloud)" cmd /k "set MANAGER_URL=%MANAGER_URL% && cargo run -p tron-zero-client"
