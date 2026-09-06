# Prepare bindgen's LLVM dependency before Cargo starts compiling asio-sys.
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
