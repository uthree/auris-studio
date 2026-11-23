# Starts Auris Studio from this checkout with the experimental generate_part tool switched on.
#
#     powershell -ExecutionPolicy Bypass -File tools\genmodel\run-auris.ps1
#
# The model environments are looked for in <main checkout>\.local-evaluations\genmodel (see
# README.md); -GenmodelHome names another folder. Extra arguments go to `cargo run`.
param(
    [string]$GenmodelHome = "",
    [switch]$DebugBuild,
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$CargoArgs
)
$ErrorActionPreference = "Stop"

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
if (-not $GenmodelHome) {
    # A worktree shares .local-evaluations with the main checkout, which owns the .git folder.
    $common = (git -C $repo rev-parse --path-format=absolute --git-common-dir).Trim()
    $GenmodelHome = Join-Path (Split-Path $common -Parent) ".local-evaluations\genmodel"
}
$python = Join-Path $GenmodelHome "ace-step\venv_rocm\Scripts\python.exe"
$script = Join-Path $PSScriptRoot "genpart.py"
foreach ($path in @($python, $script, (Join-Path $GenmodelHome "stable-audio-tools"))) {
    if (-not (Test-Path $path)) { throw "Not found: $path (set up the models as README.md describes)" }
}

$env:AURIS_GENMODEL_PYTHON = $python
$env:AURIS_GENMODEL_SCRIPT = $script
$env:AURIS_GENMODEL_HOME = $GenmodelHome
# sccache makes rustc's command line too long on Windows (os error 206).
Remove-Item Env:RUSTC_WRAPPER -ErrorAction SilentlyContinue

Write-Host "generate_part: models in $GenmodelHome"
Set-Location $repo
$profileArgs = if ($DebugBuild) { @() } else { @("--release") }
cargo run @profileArgs @CargoArgs
