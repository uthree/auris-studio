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
$venv = Join-Path $GenmodelHome "ace-step\venv_rocm"
$script = Join-Path $PSScriptRoot "genpart.py"
foreach ($path in @((Join-Path $venv "pyvenv.cfg"), $script, (Join-Path $GenmodelHome "stable-audio-tools"))) {
    if (-not (Test-Path $path)) { throw "Not found: $path (set up the models as README.md describes)" }
}
# The interpreter the venv names in pyvenv.cfg, started directly; genpart.py adds the venv's
# packages itself. The venv's own python.exe is a uv trampoline with its base interpreter's path
# built in, and a Python that uv installed from inside the Claude desktop app lives in that app's
# private copy of AppData, which nothing started outside the app can see. Point pyvenv.cfg's home
# at an interpreter outside AppData (README.md).
$homeLine = Get-Content (Join-Path $venv "pyvenv.cfg") | Where-Object { $_ -match '^\s*home\s*=' } | Select-Object -First 1
$baseDir = ($homeLine -split '=', 2)[1].Trim()
$item = Get-Item $baseDir
if ($item.LinkType) { $baseDir = @($item.Target)[0] }
$python = Join-Path $baseDir "python.exe"
if (-not (Test-Path $python)) { throw "Not found: $python" }

$env:AURIS_GENMODEL_PYTHON = $python
$env:AURIS_GENMODEL_SCRIPT = $script
$env:AURIS_GENMODEL_HOME = $GenmodelHome
# sccache makes rustc's command line too long on Windows (os error 206).
Remove-Item Env:RUSTC_WRAPPER -ErrorAction SilentlyContinue

Write-Host "generate_part: models in $GenmodelHome"
Set-Location $repo
# Built as an array explicitly: an `if` expression yielding one string would splat its characters.
$cargoArgs = @("run")
if (-not $DebugBuild) { $cargoArgs += "--release" }
if ($CargoArgs) { $cargoArgs += $CargoArgs }
Write-Host "cargo $($cargoArgs -join ' ')"
cargo @cargoArgs
