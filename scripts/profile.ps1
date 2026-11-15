# CPU profile of a local run with samply (docs/guides/dev.md, Profiling).
#
#   scripts\profile.ps1 server [-Secs 40]     server + bots, one idle human joined
#   scripts\profile.ps1 client                client window (a local server is started for it); close the window to stop
#   scripts\profile.ps1 lab [lab args...]     headless lab, default: ffa8 tournament --quick
#
# Builds with `--profile profiling` (release + line tables), records with
# `samply record --save-only` to target\profiles\<target>-<time>.json.gz (+ a
# .syms.json with the symbols) and prints how to read it. Windows needs xperf
# (Windows Performance Toolkit); samply asks for elevation on each recording.
[CmdletBinding(PositionalBinding = $false)]
param(
    [Parameter(Mandatory, Position = 0)][ValidateSet('server', 'client', 'lab')][string]$Target,
    [int]$Secs = 40,
    [Parameter(ValueFromRemainingArguments)][string[]]$Rest
)
$ErrorActionPreference = 'Stop'
Set-Location "$PSScriptRoot\.."

foreach ($tool in 'cargo', 'samply') {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { throw "$tool not on PATH (cargo install --locked samply)" }
}
New-Item -ItemType Directory -Force target\profiles | Out-Null
$out = Join-Path (Resolve-Path target\profiles).Path "$Target-$(Get-Date -Format yyyyMMdd-HHmmss).json.gz"
$bin = (Resolve-Path 'target\profiling').Path
$record = @('record', '--save-only', '--no-open', '--unstable-presymbolicate', '-o', $out)
# Samply finds a .pdb only through the bare file name the trace gives, which it resolves
# against its own working directory: it runs from the binaries' folder, so a path
# handed to the profiled program must be absolute.
function Abs($p) { if ($p -and (Test-Path $p)) { (Resolve-Path $p).Path } else { $p } }
function Record($exe, $exeArgs = @()) {
    Push-Location $bin
    try { samply @record "$bin\$exe" @exeArgs } finally { Pop-Location }
}

# Stops the profiled processes still running: only ours, by path, never a dev build's.
function Stop-Ours {
    Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($bin) -and $_.Name -match 'tron-zero-server|idle_human' } |
        Stop-Process -Force -ErrorAction SilentlyContinue
}

try {
    switch ($Target) {
        'server' {
            cargo build --profile profiling -p tron-zero-server -p tron-zero-nettest --example idle_human
            # Bots only play while a human is connected. The official config (the default)
            # keeps them riding when that human is dead, as on the deployed server.
            if ($env:SERVER_CONFIG) { $env:SERVER_CONFIG = Abs $env:SERVER_CONFIG }
            # Samply ends the recording when its child exits, so stop the server after $Secs.
            $null = Start-Job { param($wait, $human, $dir)
                Start-Sleep 5; Start-Process $human -WindowStyle Hidden
                Start-Sleep $wait
                Get-Process -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.StartsWith($dir) } | Stop-Process -Force
            } -ArgumentList $Secs, "$bin\examples\idle_human.exe", $bin
            Record tron-zero-server.exe
        }
        'client' {
            cargo build --profile profiling -p tron-zero-server -p tron-zero-client
            if (-not $env:TRON_SIM_LATENCY_MS) { $env:TRON_SIM_LATENCY_MS = '20' }
            $env:TRON_AUTOCONNECT = '127.0.0.1:5000'
            if (-not $env:SERVER_CONFIG) { $env:SERVER_CONFIG = 'crates\server\config\local.toml' }
            $env:SERVER_CONFIG = Abs $env:SERVER_CONFIG
            Start-Process "$bin\tron-zero-server.exe" -WindowStyle Hidden
            Record tron-zero-client.exe
        }
        'lab' {
            cargo build --profile profiling -p tron-zero-lab
            if (-not $Rest) { $Rest = 'tournament', 'crates\lab\suites\ffa8.toml', '--quick' }
            Record tron-zero-lab.exe ($Rest | ForEach-Object { Abs $_ })
        }
    }
}
finally {
    Stop-Ours
    Get-Job | Remove-Job -Force
}

Write-Host "Profile: $out"
Write-Host "Browser: samply load $out"
Write-Host "Text:    profiler-cli load $out --with-samply; profiler-cli thread samples"
