param(
    [switch]$Run
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$terrainManifest = Join-Path (Split-Path $projectRoot -Parent) 'bevy_terrain\Cargo.toml'

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'Cargo was not found. Install Rust with rustup (MSVC toolchain) and restart PowerShell.'
}

if (-not (Test-Path $terrainManifest -PathType Leaf)) {
    throw "Missing path dependency: $terrainManifest. Cargo.toml requires ../bevy_terrain; place that project next to world_game before building."
}

Push-Location $projectRoot
try {
    Write-Host 'Building world-client for Windows...'
    & cargo build --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE"
    }

    if ($Run) {
        Write-Host 'Starting world-client...'
        & cargo run --locked --bin world-client
        if ($LASTEXITCODE -ne 0) {
            throw "cargo run failed with exit code $LASTEXITCODE"
        }
    }
}
finally {
    Pop-Location
}
