# Generate fixtures with the historical model serializers, without checking out over local work.
$ErrorActionPreference = 'Stop'
$taskRepo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$taskFixtures = Join-Path $taskRepo 'crates/auris-io/tests/fixtures/project_formats'
$taskScratch = Join-Path ([System.IO.Path]::GetTempPath()) ('auris-format-fixtures-' + [guid]::NewGuid())
$taskRevisions = [ordered]@{
    27 = '85e77d272851cb685879e23ea66d6d2fe720edff'
    28 = '83c2affec13db541900b9402a2bb504c9d3eb049'
    29 = '909f2bc827598db3113c179ed607087817bfc31b'
    30 = 'c828ab0dd16cab0aae6ee4f04cd1629e8e55d0c5'
    31 = '61121f120af41de5c8400f9d5c856a3a40d45e47'
    32 = '730493ada12802b0bc7bfda42d41c07db3c74081'
}
$taskManifest = @'
[package]
name = "historical-project-fixture-author"
version = "0.0.0"
edition = "2024"
[features]
multi-output = []
[workspace]
members = ["crates/auris-core"]
resolver = "3"
[workspace.package]
version = "0.0.0"
edition = "2024"
license = "Apache-2.0"
repository = "https://github.com/uthree/auris-studio"
rust-version = "1.95"
[workspace.dependencies]
base64 = "=0.22.1"
serde = { version = "=1.0.229", features = ["derive"] }
serde_json = "=1.0.154"
thiserror = "=2.0.21"
[dependencies]
auris-core = { path = "crates/auris-core" }
serde_json.workspace = true
'@
New-Item -ItemType Directory -Path $taskScratch | Out-Null
foreach ($taskEntry in $taskRevisions.GetEnumerator()) {
    $taskRevision = (git -C $taskRepo rev-parse $taskEntry.Value).Trim()
    if ($LASTEXITCODE -ne 0) { throw 'Historical revision is unavailable' }
    $taskArchive = Join-Path $taskScratch ($taskEntry.Key.ToString() + '.zip')
    git -C $taskRepo archive --format=zip --output=$taskArchive $taskRevision crates/auris-core
    if ($LASTEXITCODE -ne 0) { throw "Cannot archive $taskRevision" }
    $taskBuild = Join-Path $taskScratch $taskEntry.Key.ToString()
    Expand-Archive -LiteralPath $taskArchive -DestinationPath $taskBuild
    Set-Content -LiteralPath (Join-Path $taskBuild 'Cargo.toml') -Value $taskManifest -Encoding utf8
    New-Item -ItemType Directory -Path (Join-Path $taskBuild 'src') | Out-Null
    Copy-Item -LiteralPath (Join-Path $taskFixtures 'generate.rs') -Destination (Join-Path $taskBuild 'src/main.rs')
    $taskOutput = Join-Path $taskFixtures ('v' + $taskEntry.Key + '.auris')
    $taskFeatures = @()
    if ($taskEntry.Key -ge 32) { $taskFeatures = @('--features', 'multi-output') }
    # Archives retain old mtimes; isolate targets so a previous revision's binary cannot be reused.
    cargo run @taskFeatures --offline --manifest-path (Join-Path $taskBuild 'Cargo.toml') --target-dir (Join-Path $taskBuild 'target') -- $taskOutput
    if ($LASTEXITCODE -ne 0) { throw "Fixture generation failed for format $($taskEntry.Key)" }
    $taskStored = Get-Content -LiteralPath $taskOutput -Raw | ConvertFrom-Json
    if ($taskStored.format_version -ne $taskEntry.Key) { throw 'Unexpected historical format version' }
    Write-Output "Generated format $($taskEntry.Key) from $taskRevision"
}
Write-Output "Historical build sources remain in $taskScratch; they are not runtime dependencies."
