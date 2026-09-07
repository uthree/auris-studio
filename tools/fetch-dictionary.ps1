# Install the same pinned Japanese dictionary as fetch-dictionary.sh, without requiring Bash.
[CmdletBinding()]
param(
    [string]$Destination = (Join-Path (Split-Path -Parent $PSScriptRoot) 'Dictionary'),
    [string]$AurisPath
)

$ErrorActionPreference = 'Stop'
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
if ($AurisPath) {
    $manifest = @(& $AurisPath dictionary --manifest)
} else {
    Push-Location (Split-Path -Parent $PSScriptRoot)
    try {
        $manifest = @(& cargo run --quiet --locked -p auris-cli -- dictionary --manifest)
    } finally {
        Pop-Location
    }
}
if ($LASTEXITCODE -ne 0 -or $manifest.Count -ne 1) {
    throw 'Could not read the dictionary manifest from auris.'
}
$fields = $manifest[0] -split "`t"
if ($fields.Count -ne 6) { throw 'Invalid dictionary manifest.' }
$id, $folder, $bytes, $sha256, $url, $licenseUrl = $fields
if ($folder -notmatch '^[a-zA-Z0-9_-]+$' -or $sha256 -notmatch '^[a-f0-9]{64}$') {
    throw 'Invalid dictionary folder or SHA-256 in the manifest.'
}
$expectedLength = [uint64]::Parse($bytes)
$target = Join-Path $destinationPath $folder
$notice = Join-Path $destinationPath "${folder}_License.md"
$marker = Join-Path $target 'metadata.json'
if ((Test-Path -LiteralPath $marker -PathType Leaf) -and
    (Test-Path -LiteralPath $notice -PathType Leaf)) {
    Write-Host "${id}: already installed at $target"
    return
}

New-Item -ItemType Directory -Force -Path $destinationPath | Out-Null
$staging = Join-Path $destinationPath ('.dictionary-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $staging | Out-Null
try {
    $noticeDownload = Join-Path $staging 'License.md'
    Invoke-WebRequest -Uri $licenseUrl -OutFile $noticeDownload -TimeoutSec 600
    # A previously installed dictionary may only be missing its notice.
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) {
        if (Test-Path -LiteralPath $target) {
            throw "Incomplete dictionary at '$target'. Move it aside before retrying."
        }
        $archive = Join-Path $staging 'dictionary.tar.gz'
        Write-Host "${id}: downloading $expectedLength bytes"
        Invoke-WebRequest -Uri $url -OutFile $archive -TimeoutSec 600
        if ((Get-Item -LiteralPath $archive).Length -ne $expectedLength) {
            throw 'Dictionary archive length differs from the manifest.'
        }
        if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $sha256) {
            throw 'Dictionary archive SHA-256 differs from the manifest.'
        }
        # Only the verified archive is extracted, into an isolated directory on the same volume.
        & tar -xzf $archive -C $staging
        if ($LASTEXITCODE -ne 0) { throw 'Could not extract the dictionary archive.' }
        $extracted = Join-Path $staging $folder
        if (-not (Test-Path -LiteralPath (Join-Path $extracted 'metadata.json') -PathType Leaf)) {
            throw "Archive did not contain $folder/metadata.json."
        }
        if (-not (Test-Path -LiteralPath $notice)) {
            Move-Item -LiteralPath $noticeDownload -Destination $notice
        }
        # Check the resolved paths before publishing a directory. Directory.Move refuses an
        # existing target instead of nesting our folder inside a concurrent installation.
        $prefix = $destinationPath.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
        foreach ($candidate in @($extracted, $target)) {
            if (-not [System.IO.Path]::GetFullPath($candidate).StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw 'Refusing to move a dictionary outside the destination.'
            }
        }
        try {
            [System.IO.Directory]::Move($extracted, $target)
        } catch {
            if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) { throw }
        }
    } elseif (-not (Test-Path -LiteralPath $notice)) {
        Move-Item -LiteralPath $noticeDownload -Destination $notice
    }
    Write-Host "${id}: installed at $target"
} finally {
    $resolvedStaging = [System.IO.Path]::GetFullPath($staging)
    $prefix = $destinationPath.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $resolvedStaging.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Refusing to remove a staging directory outside the destination.'
    }
    Remove-Item -LiteralPath $resolvedStaging -Recurse -Force
}
