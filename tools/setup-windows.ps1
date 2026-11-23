# Prepare what asio-sys needs before Cargo starts compiling it: LLVM for bindgen, and the ASIO SDK.
# Environment changes apply to this PowerShell session and subsequent GitHub Actions steps.
[CmdletBinding()]
param(
    [switch]$InstallLlvm,
    [string]$LlvmBin = $env:LIBCLANG_PATH
)

$ErrorActionPreference = 'Stop'

if ($env:OS -ne 'Windows_NT') {
    throw 'This script prepares Windows builds only.'
}

function Find-LibclangDirectory {
    if ($LlvmBin) {
        # clang-sys also accepts LIBCLANG_PATH as the full DLL filename.
        if ((Test-Path -LiteralPath $LlvmBin -PathType Leaf) -and
            (Split-Path -Leaf $LlvmBin) -in @('libclang.dll', 'clang.dll')) {
            return Split-Path -Parent (Resolve-Path -LiteralPath $LlvmBin).Path
        }
        if (-not ((Test-Path -LiteralPath (Join-Path $LlvmBin 'libclang.dll') -PathType Leaf) -or
            (Test-Path -LiteralPath (Join-Path $LlvmBin 'clang.dll') -PathType Leaf))) {
            throw "libclang.dll was not found in '$LlvmBin'. Correct LIBCLANG_PATH or pass -LlvmBin with LLVM's bin directory."
        }
        return (Resolve-Path -LiteralPath $LlvmBin).Path
    }

    $candidates = @(
        (Join-Path $env:ProgramFiles 'LLVM\bin')
        ($env:PATH -split ';' | Where-Object { $_ })
    )
    foreach ($candidate in $candidates) {
        if ((Test-Path -LiteralPath (Join-Path $candidate 'libclang.dll') -PathType Leaf) -or
            (Test-Path -LiteralPath (Join-Path $candidate 'clang.dll') -PathType Leaf)) {
            return (Resolve-Path -LiteralPath $candidate).Path
        }
    }
}

$clangDirectory = Find-LibclangDirectory
if (-not $clangDirectory -and $InstallLlvm) {
    if (Get-Command choco.exe -ErrorAction SilentlyContinue) {
        & choco.exe install llvm --yes --no-progress
    } elseif (Get-Command winget.exe -ErrorAction SilentlyContinue) {
        & winget.exe install --exact --id LLVM.LLVM --source winget --silent --accept-package-agreements --accept-source-agreements
    } else {
        throw 'Install LLVM from https://releases.llvm.org/ and rerun with -LlvmBin pointing to its bin directory.'
    }
    if ($LASTEXITCODE -ne 0) {
        throw "LLVM installation failed (exit code $LASTEXITCODE)."
    }
    $clangDirectory = Find-LibclangDirectory
}

if (-not $clangDirectory) {
    throw 'ASIO requires LLVM libclang.dll in addition to the Visual Studio C++ tools. Run .\tools\setup-windows.ps1 -InstallLlvm, or pass -LlvmBin for an existing LLVM installation.'
}

$env:LIBCLANG_PATH = $clangDirectory
if ($env:GITHUB_ENV) {
    "LIBCLANG_PATH=$clangDirectory" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
Write-Host "ASIO bindings will use LLVM in $clangDirectory"

# Fetch the ASIO SDK here rather than leave it to asio-sys. Its build script extracts the
# archive by starting Windows PowerShell 5.1, which inherits PowerShell 7's PSModulePath when
# Cargo runs under pwsh (as every GitHub Actions step does), loads 7's Archive module, and fails
# with "Cannot find an overload for ExtractToDirectory". It then builds against an empty
# directory. The default location is the one asio-sys itself looks in, so a build started
# without CPAL_ASIO_DIR finds it as well.
function Test-AsioSdk([string]$Directory) {
    $Directory -and (Test-Path -LiteralPath (Join-Path $Directory 'host\asiodrivers.h') -PathType Leaf) -and
        (Test-Path -LiteralPath (Join-Path $Directory 'common\asio.h') -PathType Leaf)
}

$asioDirectory = $env:CPAL_ASIO_DIR
if (-not (Test-AsioSdk $asioDirectory)) {
    if ($asioDirectory) {
        throw "The ASIO SDK was not found in CPAL_ASIO_DIR='$asioDirectory'. Correct it, or unset it to download the SDK."
    }
    $asioDirectory = Join-Path ([System.IO.Path]::GetTempPath()) 'asio_sdk'
}
if (-not (Test-AsioSdk $asioDirectory)) {
    # A failed extraction by asio-sys leaves this directory behind, empty or partial.
    if (Test-Path -LiteralPath $asioDirectory) {
        Remove-Item -LiteralPath $asioDirectory -Recurse -Force
    }
    $staging = Join-Path ([System.IO.Path]::GetTempPath()) "asio_sdk_download_$PID"
    $archive = "$staging.zip"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        $ProgressPreference = 'SilentlyContinue'
        Invoke-WebRequest -Uri 'https://www.steinberg.net/asiosdk' -OutFile $archive -UseBasicParsing
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [System.IO.Compression.ZipFile]::ExtractToDirectory($archive, $staging)
        # The archive holds one versioned folder, such as asiosdk_2.3.3_2019-06-14.
        $root = Get-ChildItem -LiteralPath $staging -Directory | Where-Object { Test-AsioSdk $_.FullName } |
            Select-Object -First 1
        if (-not $root) {
            throw 'The downloaded ASIO SDK archive does not hold host\asiodrivers.h and common\asio.h.'
        }
        Move-Item -LiteralPath $root.FullName -Destination $asioDirectory
    } finally {
        Remove-Item -LiteralPath $staging, $archive -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$env:CPAL_ASIO_DIR = $asioDirectory
if ($env:GITHUB_ENV) {
    "CPAL_ASIO_DIR=$asioDirectory" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
Write-Host "ASIO bindings will use the ASIO SDK in $asioDirectory"
