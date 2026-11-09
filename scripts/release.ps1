<#
.SYNOPSIS
  Build the Windows executables and publish a GitHub Release that the
  desktop app's updater will offer to users.

.DESCRIPTION
  1. Bump `version` in the workspace Cargo.toml and commit/push it to main.
  2. Run from a shell where the MSVC x64 build tools work (e.g. "x64 Native
     Tools Command Prompt for VS"), with `gh` logged in as the repo owner:
       ./scripts/release.ps1 -Version 0.1.1 -Notes "What changed"
  The script builds strata.exe and strata-mcp.exe with a static C runtime,
  packs them into the Windows installer (installer/strata.iss, built with
  Inno Setup 6), writes a .sha256 file next to each download (the updater
  refuses to install without it), and creates the release with tag
  v<Version> on main.

  Inno Setup: `winget install JRSoftware.InnoSetup --scope user`, or pass
  -Iscc <path to ISCC.exe>.

  Asset names are part of the updater's contract (see
  crates/strata-app/src/updater.rs): strata-v<Version>-windows-x64.exe and
  strata-v<Version>-windows-x64.exe.sha256. The installer is
  strata-v<Version>-windows-x64-setup.exe.
#>
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$Notes = "",
    [string]$Iscc = ""
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

if (-not $Iscc) {
    $candidates = @(
        (Get-Command ISCC.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source),
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
    )
    $Iscc = $candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $Iscc) { throw "Inno Setup 6 (ISCC.exe) not found. Install it: winget install JRSoftware.InnoSetup --scope user" }

$cargoVersion = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
if ($cargoVersion -ne $Version) {
    throw "Cargo.toml says version $cargoVersion. Bump it to $Version (and push) before releasing."
}
if (git status --porcelain) { throw "The working tree has uncommitted changes. Commit or stash them first." }
$head = (git rev-parse HEAD).Trim()
$remote = (git ls-remote origin refs/heads/main).Split()[0]
if ($head -ne $remote) { throw "HEAD ($head) is not what origin/main has ($remote). Push first." }

$env:RUSTFLAGS = "-C target-feature=+crt-static"
cargo build --release --target x86_64-pc-windows-msvc -p strata-app -p strata-mcp
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

$out = Join-Path $root "target/release-assets/v$Version"
New-Item -ItemType Directory -Force $out | Out-Null
$files = @(
    @{ From = "target/x86_64-pc-windows-msvc/release/strata.exe"; Name = "strata-v$Version-windows-x64.exe" },
    @{ From = "target/x86_64-pc-windows-msvc/release/strata-mcp.exe"; Name = "strata-mcp-v$Version-windows-x64.exe" }
)
$release = Join-Path $root "target/x86_64-pc-windows-msvc/release"
& $Iscc /Qp "/DAppVersion=$Version" "/DSourceDir=$release" "/DOutputDir=$out" (Join-Path $root "installer/strata.iss")
if ($LASTEXITCODE -ne 0) { throw "building the installer failed" }
$setup = "strata-v$Version-windows-x64-setup.exe"

$assets = @()
foreach ($f in $files + @(@{ From = (Join-Path $out $setup); Name = $setup })) {
    $dest = Join-Path $out $f.Name
    if ((Resolve-Path $f.From).Path -ne [IO.Path]::GetFullPath($dest)) { Copy-Item $f.From $dest -Force }
    $hash = (Get-FileHash $dest -Algorithm SHA256).Hash.ToLower()
    Set-Content -Path "$dest.sha256" -Value "$hash  $($f.Name)" -NoNewline -Encoding ascii
    $assets += $dest, "$dest.sha256"
}

if (-not $Notes) { $Notes = "Strata v$Version" }
gh release create "v$Version" @assets --repo erivgout/strata-sc2-editor --title "Strata v$Version" --notes $Notes --target main
if ($LASTEXITCODE -ne 0) { throw "gh release create failed" }
Write-Host "Released v${Version}: https://github.com/erivgout/strata-sc2-editor/releases/tag/v$Version"
