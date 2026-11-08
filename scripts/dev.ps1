param(
    [ValidateSet('test', 'check', 'build', 'fmt', 'clippy')]
    [string]$Action = 'test'
)
$ErrorActionPreference = 'Stop'
$taskRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $taskRoot
try {
    $taskCargo = Join-Path $taskRoot '.tools/cargo/bin/cargo.exe'
    if (Test-Path -LiteralPath $taskCargo) {
        $env:CARGO_HOME = Join-Path $taskRoot '.tools/cargo'
        $env:RUSTUP_HOME = Join-Path $taskRoot '.tools/rustup'
    } else {
        $taskCargo = 'cargo'
    }
    switch ($Action) {
        test { & $taskCargo test --workspace --locked }
        check { & $taskCargo check --workspace --locked }
        build { & $taskCargo build --workspace --release --locked }
        fmt { & $taskCargo fmt --all -- --check }
        clippy { & $taskCargo clippy --workspace --all-targets --locked -- -D warnings }
    }
    if ($LASTEXITCODE -ne 0) { throw "Rust $Action failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}
