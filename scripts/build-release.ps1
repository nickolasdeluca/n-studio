<#
.SYNOPSIS
    Builds the Windows installer of N Studio: the NSIS setup, x64.

.DESCRIPTION
    The Windows side of build-release.sh, for a Windows host. It builds what
    the release workflow builds, here, and leaves it in dist\ with its
    sha256.

    The installer is NOT signed: there is no Authenticode certificate, so
    SmartScreen still asks before it runs (the README says what to answer).
    macOS is the one system with a signed and notarized build, and that is
    build-release.sh on a Mac.

    Needs Rust (the MSVC toolchain) and Node on PATH; the WebView2 runtime
    is fetched by the installer on the machine it is installed on.

    Config file: scripts\build-release.env (gitignored; copy
    build-release.env.example beside it) is read when it is there. It is the
    file build-release.sh reads; only DIST_DIR matters here, and the macOS
    signing keys in it are ignored. Variables already set in the calling
    shell win over the file.

.PARAMETER DistDir
    Where the installer is copied. Default: dist (or $env:DIST_DIR).

.EXAMPLE
    .\scripts\build-release.ps1
#>

[CmdletBinding()]
param(
    [string]$DistDir
)

$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

function Log([string]$Message) {
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Blue
}
function Warn([string]$Message) {
    Write-Host "!! $Message" -ForegroundColor Yellow
}
function Die([string]$Message) {
    Write-Host "ERROR: $Message" -ForegroundColor Red
    exit 1
}

if ($env:OS -ne "Windows_NT") {
    Die "This builds the Windows installer and runs on Windows. On a Mac, use scripts/build-release.sh."
}

# The file build-release.sh sources, read as KEY=VALUE lines. A value the
# caller already has in the environment is kept.
$EnvFile = Join-Path $RepoRoot "scripts\build-release.env"
if (Test-Path $EnvFile) {
    foreach ($line in Get-Content $EnvFile) {
        $trimmed = $line.Trim()
        if ($trimmed -eq "" -or $trimmed.StartsWith("#")) { continue }
        if ($trimmed -notmatch '^([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') { continue }
        $name = $matches[1]
        $value = $matches[2].Trim()
        if ($value.Length -ge 2 -and (
            ($value.StartsWith('"') -and $value.EndsWith('"')) -or
            ($value.StartsWith("'") -and $value.EndsWith("'"))
        )) {
            $value = $value.Substring(1, $value.Length - 2)
        }
        if (-not (Test-Path "env:$name")) {
            Set-Item -Path "env:$name" -Value $value
        }
    }
}

if (-not $DistDir) {
    $DistDir = if ($env:DIST_DIR) { $env:DIST_DIR } else { "dist" }
}

# The bundles are named after the workspace's version, as in the workflow
$Version = $null
$inWorkspacePackage = $false
foreach ($line in Get-Content (Join-Path $RepoRoot "Cargo.toml")) {
    if ($line -match '^\[(.+)\]\s*$') {
        $inWorkspacePackage = ($matches[1] -eq "workspace.package")
        continue
    }
    if ($inWorkspacePackage -and $line -match '^version\s*=\s*"([^"]+)"') {
        $Version = $matches[1]
        break
    }
}
if (-not $Version) { Die "No version in Cargo.toml's [workspace.package]." }

foreach ($tool in @("cargo", "npm")) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Die "$tool is not on PATH." }
}

Log "N Studio $Version for Windows x64"

Push-Location (Join-Path $RepoRoot "ui")
try {
    & npm ci
    if ($LASTEXITCODE -ne 0) { Die "npm ci failed." }
}
finally {
    Pop-Location
}

# The Tauri CLI itself, from the repository's root, rather than
# `npm run tauri -- build`: PowerShell's npm shim swallows the `--`.
$Tauri = Join-Path $RepoRoot "ui\node_modules\.bin\tauri.cmd"
if (-not (Test-Path $Tauri)) { Die "The Tauri CLI is not in ui\node_modules; npm ci should have put it there." }

Log "Building"
& $Tauri build --bundles nsis
if ($LASTEXITCODE -ne 0) { Die "tauri build failed." }

$Installer = Join-Path $RepoRoot "target\release\bundle\nsis\N Studio_${Version}_x64-setup.exe"
if (-not (Test-Path -LiteralPath $Installer)) { Die "The build left no $Installer." }

Warn "The installer is not signed (no Authenticode certificate): SmartScreen will ask before it runs."

New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
Copy-Item -LiteralPath $Installer -Destination $DistDir -Force
$Out = Get-Item -LiteralPath (Join-Path $DistDir (Split-Path -Leaf $Installer))

Log "Done: $($Out.FullName)"
Write-Host "    sha256: $((Get-FileHash -LiteralPath $Out.FullName -Algorithm SHA256).Hash.ToLower())"
Write-Host "    bytes:  $($Out.Length)"
