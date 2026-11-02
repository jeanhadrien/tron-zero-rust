#requires -Version 7
<#
.SYNOPSIS
  Builds the browser client into web/dist (wasm + JS glue + index.html).

.DESCRIPTION
  1. cargo build --profile wasm-release --target wasm32-unknown-unknown
  2. wasm-bindgen --target web   (CLI version must equal the wasm-bindgen crate in Cargo.lock)
  3. wasm-opt -O3                (skipped with -SkipOpt)
  4. copy web/index.html

  One-time setup:
    rustup target add wasm32-unknown-unknown
    cargo install wasm-bindgen-cli --version <version from Cargo.lock> --locked
    npm i -g binaryen            # provides wasm-opt (or: cargo install wasm-opt)

  Set TRONZERO_MANAGER_URL to bake the lobby URL into the build.
#>
param([switch]$SkipOpt)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$dist = Join-Path $PSScriptRoot 'dist'
$name = 'tron-zero-client'
Set-Location $root

$lockVersion = (Select-String -Path Cargo.lock -Pattern '^name = "wasm-bindgen"$' -Context 0, 1 |
    Select-Object -First 1).Context.PostContext[0] -replace '[^0-9.]', ''
$cliVersion = ((wasm-bindgen --version) -split ' ')[1]
if ($cliVersion -ne $lockVersion) {
    throw "wasm-bindgen CLI is $cliVersion but Cargo.lock has $lockVersion. Run: cargo install wasm-bindgen-cli --version $lockVersion --locked"
}

cargo build --profile wasm-release --target wasm32-unknown-unknown -p tron-zero-client
if ($LASTEXITCODE) { throw 'cargo build failed' }

if (Test-Path $dist) { Remove-Item $dist -Recurse -Force }
New-Item -ItemType Directory $dist | Out-Null

wasm-bindgen --target web --no-typescript --out-dir $dist --out-name $name `
    "target/wasm32-unknown-unknown/wasm-release/$name.wasm"
if ($LASTEXITCODE) { throw 'wasm-bindgen failed' }

$wasm = Join-Path $dist "${name}_bg.wasm"
if (-not $SkipOpt) {
    # Explicit features: --all-features also enables ones browsers reject
    # (e.g. compact imports). These are what rustc emits by default.
    wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext `
        --enable-mutable-globals --enable-multivalue --enable-reference-types -o $wasm $wasm
    if ($LASTEXITCODE) { throw 'wasm-opt failed' }
}

Copy-Item (Join-Path $PSScriptRoot 'index.html') $dist
$size = (Get-Item $wasm).Length
"{0}: {1:N1} MB raw" -f (Split-Path $wasm -Leaf), ($size / 1MB)
