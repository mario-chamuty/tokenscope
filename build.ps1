#!/usr/bin/env pwsh
<#
.SYNOPSIS
    Build TokenScope installers for Windows, Linux and macOS into ./dist.

.DESCRIPTION
    Windows : built natively on this host          -> .exe (NSIS) + .msi
    Linux   : built in a Docker Linux container     -> .deb + .AppImage
    macOS   : built in a KVM-backed macOS VM (dockur/macos) over SSH -> .dmg

    macOS requires a ONE-TIME VM setup (install macOS + toolchain once, see
    docker/macos/SETUP.md). After that, builds are fully automated from here.

.EXAMPLE
    ./build.ps1                 # build every target the machine can build
    ./build.ps1 -Target linux   # just Linux
    ./build.ps1 -Target win,linux
    ./build.ps1 -Clean          # wipe dist first
    ./build.ps1 -Rebuild        # force-rebuild the Linux Docker image
    ./build.ps1 -Provision      # launch the macOS VM viewer for one-time setup
#>
[CmdletBinding()]
param(
    [ValidateSet('all', 'win', 'windows', 'linux', 'mac', 'macos')]
    [string[]]$Target = @('all'),
    [switch]$Clean,
    [switch]$Rebuild,
    [switch]$Provision,
    [int]$MacSshPort = 2222,
    [int]$BootTimeoutSec = 900
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Root        = $PSScriptRoot
$Dist        = Join-Path $Root 'dist'
$TauriConf   = Join-Path $Root 'src-tauri/tauri.conf.json'
$LinuxImage  = 'clatok-linux-builder'
$MacImage    = 'dockurr/macos'
$MacName     = 'clatok-macos'
$MacStorage  = Join-Path $Root 'docker/macos/storage'
$MacKey      = Join-Path $Root 'docker/macos/id_ed25519'

# --- helpers ---------------------------------------------------------------

function Write-Step([string]$msg) { Write-Host "`n=== $msg ===" -ForegroundColor Cyan }
function Write-Ok([string]$msg)   { Write-Host "  OK  $msg"      -ForegroundColor Green }
function Write-Warn2([string]$msg){ Write-Host "  !!  $msg"      -ForegroundColor Yellow }

function Test-Tool([string]$name) {
    return [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

# Docker on Windows wants forward-slash host paths; drive letter is special-cased.
function ConvertTo-DockerPath([string]$p) {
    return ((Resolve-Path $p).Path -replace '\\', '/')
}

function Invoke-Native {
    param([string]$Exe, [string[]]$CmdArgs, [string]$What)
    & $Exe @CmdArgs
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)." }
}

function Get-Conf {
    $json = Get-Content $TauriConf -Raw | ConvertFrom-Json
    return [pscustomobject]@{ Product = $json.productName; Version = $json.version }
}

# --- Windows ---------------------------------------------------------------

function Build-Windows {
    Write-Step 'Windows (native)'
    $conf = Get-Conf

    if (-not (Test-Path (Join-Path $Root 'node_modules/@tauri-apps/cli'))) {
        Write-Warn2 'Tauri CLI not found in node_modules - running npm install...'
        Invoke-Native 'npm' @('install') 'npm install'
    }

    Push-Location $Root
    try {
        Invoke-Native 'npm' @('run', 'tauri', '--', 'build') 'tauri build (Windows)'
    } finally { Pop-Location }

    $bundle = Join-Path $Root 'src-tauri/target/release/bundle'
    $copied = 0
    foreach ($pat in @("nsis/*$($conf.Version)*-setup.exe", "msi/*$($conf.Version)*.msi")) {
        Get-ChildItem (Join-Path $bundle $pat) -ErrorAction SilentlyContinue | ForEach-Object {
            Copy-Item $_.FullName $Dist -Force
            Write-Ok "dist/$($_.Name)"
            $copied++
        }
    }
    if ($copied -eq 0) { throw 'Windows build produced no installers.' }
}

# --- Linux -----------------------------------------------------------------

function Ensure-LinuxImage {
    & docker image inspect $LinuxImage *> $null
    $exists = ($LASTEXITCODE -eq 0)
    if ($Rebuild -or -not $exists) {
        Write-Step "Building Docker image '$LinuxImage'"
        Invoke-Native 'docker' @('build', '-t', $LinuxImage, (Join-Path $Root 'docker/linux')) 'docker build (linux)'
    }
}

function Build-Linux {
    Write-Step 'Linux (Docker)'
    if (-not (Test-Tool 'docker')) { throw 'docker not found on PATH.' }
    Ensure-LinuxImage

    $srcMount = "$(ConvertTo-DockerPath $Root):/src:ro"
    $outMount = "$(ConvertTo-DockerPath $Dist):/out"

    Invoke-Native 'docker' @(
        'run', '--rm',
        '-v', $srcMount,
        '-v', 'clatok-linux-work:/work',
        '-v', 'clatok-cargo-registry:/usr/local/cargo/registry',
        '-v', $outMount,
        $LinuxImage
    ) 'docker run (linux build)'

    Get-ChildItem (Join-Path $Dist '*') -Include '*.deb', '*.AppImage' -File -ErrorAction SilentlyContinue |
        ForEach-Object { Write-Ok "dist/$($_.Name)" }
}

# --- macOS -----------------------------------------------------------------

function Ssh-Mac {
    param([string[]]$Remote)
    return @(
        '-i', $MacKey, '-p', "$MacSshPort",
        '-o', 'StrictHostKeyChecking=no',
        '-o', 'UserKnownHostsFile=/dev/null',
        '-o', 'LogLevel=ERROR',
        'user@localhost'
    ) + $Remote
}

function Wait-MacSsh {
    Write-Host '  waiting for macOS VM SSH (this can take several minutes on cold boot)...'
    $deadline = (Get-Date).AddSeconds($BootTimeoutSec)
    while ((Get-Date) -lt $deadline) {
        & ssh @(Ssh-Mac @('-o', 'ConnectTimeout=5', 'echo ready')) 2>$null | Out-Null
        if ($LASTEXITCODE -eq 0) { Write-Ok 'SSH is up.'; return }
        Start-Sleep -Seconds 10
    }
    throw "macOS VM did not become reachable over SSH within $BootTimeoutSec s."
}

function Start-MacGui {
    # One-time setup: boot with the web viewer at http://localhost:8006
    Write-Step 'macOS provisioning VM (web viewer: http://localhost:8006)'
    New-Item -ItemType Directory -Force -Path $MacStorage | Out-Null
    if (-not (Test-Path $MacKey)) {
        Write-Host '  generating SSH key for the VM...'
        Invoke-Native 'ssh-keygen' @('-t', 'ed25519', '-N', '', '-f', $MacKey, '-C', 'clatok-build') 'ssh-keygen'
    }
    Write-Host "  public key to install in the VM (docker/macos/SETUP.md step 4):" -ForegroundColor Yellow
    Write-Host ("    " + (Get-Content "$MacKey.pub" -Raw).Trim())
    & docker rm -f $MacName 2>$null | Out-Null
    Invoke-Native 'docker' @(
        'run', '-d', '--name', $MacName,
        '--device=/dev/kvm', '--device=/dev/net/tun', '--cap-add', 'NET_ADMIN',
        '-p', '8006:8006', '-p', "$MacSshPort`:22",
        '-v', "$(ConvertTo-DockerPath $MacStorage):/storage",
        '--stop-timeout', '120',
        $MacImage
    ) 'docker run (macOS provision)'
    Write-Host ''
    Write-Host 'Open http://localhost:8006 and follow docker/macos/SETUP.md.' -ForegroundColor Yellow
    Write-Host "When done: docker stop $MacName" -ForegroundColor Yellow
}

function Build-Macos {
    Write-Step 'macOS (Docker VM)'
    if (-not (Test-Tool 'docker')) { throw 'docker not found on PATH.' }
    if (-not (Test-Tool 'ssh') -or -not (Test-Tool 'scp')) { throw 'OpenSSH client (ssh/scp) not found.' }

    if (-not (Test-Path (Join-Path $MacStorage 'data.img')) -and
        -not (Test-Path (Join-Path $MacStorage 'mac_hdd.img'))) {
        Write-Warn2 'No provisioned macOS disk found in docker/macos/storage.'
        Write-Warn2 'Run one-time setup first:  ./build.ps1 -Provision   (see docker/macos/SETUP.md)'
        Write-Warn2 'Skipping macOS target.'
        return
    }
    if (-not (Test-Path $MacKey)) {
        throw "SSH key $MacKey missing. Re-run setup (docker/macos/SETUP.md) to generate and install it."
    }

    # Boot the persisted VM headless (no need to open the web viewer).
    & docker rm -f $MacName 2>$null | Out-Null
    Invoke-Native 'docker' @(
        'run', '-d', '--rm', '--name', $MacName,
        '--device=/dev/kvm', '--device=/dev/net/tun', '--cap-add', 'NET_ADMIN',
        '-p', "$MacSshPort`:22",
        '-v', "$(ConvertTo-DockerPath $MacStorage):/storage",
        '--stop-timeout', '120',
        $MacImage
    ) 'docker run (macOS build VM)'

    try {
        Wait-MacSsh

        Write-Host '  packing source...'
        $tar = Join-Path $env:TEMP 'clatok-src.tgz'
        if (Test-Path $tar) { Remove-Item $tar -Force }
        Invoke-Native 'tar' @(
            '-czf', $tar, '-C', $Root,
            '--exclude=.git', '--exclude=node_modules', '--exclude=dist',
            '--exclude=src-tauri/target', '--exclude=docker/macos/storage', '.'
        ) 'tar (mac source)'

        Write-Host '  uploading + unpacking on VM...'
        & ssh @(Ssh-Mac @('rm -rf ~/clatok && mkdir -p ~/clatok')) | Out-Null
        Invoke-Native 'scp' @('-i', $MacKey, '-P', "$MacSshPort",
            '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null',
            $tar, 'user@localhost:/tmp/clatok-src.tgz') 'scp (upload source)'
        Invoke-Native 'ssh' (Ssh-Mac @('tar -xzf /tmp/clatok-src.tgz -C ~/clatok')) 'untar on VM'

        Write-Host '  building .dmg inside the VM...'
        Invoke-Native 'ssh' (Ssh-Mac @('bash ~/clatok/docker/macos/build-remote.sh')) 'tauri build (macOS)'

        Write-Host '  downloading .dmg...'
        Invoke-Native 'scp' @('-i', $MacKey, '-P', "$MacSshPort",
            '-o', 'StrictHostKeyChecking=no', '-o', 'UserKnownHostsFile=/dev/null',
            'user@localhost:~/clatok/src-tauri/target/universal-apple-darwin/release/bundle/dmg/*.dmg',
            $Dist) 'scp (download dmg)'

        Get-ChildItem (Join-Path $Dist '*') -Include '*.dmg' -File -ErrorAction SilentlyContinue |
            ForEach-Object { Write-Ok "dist/$($_.Name)" }
    } finally {
        & docker stop $MacName 2>$null | Out-Null
    }
}

# --- main ------------------------------------------------------------------

if ($Provision) { Start-MacGui; return }

$want = if ($Target -contains 'all') { @('win', 'linux', 'mac') }
        else { $Target | ForEach-Object { switch ($_) { 'windows' {'win'} 'macos' {'mac'} default {$_} } } | Select-Object -Unique }

if ($Clean -and (Test-Path $Dist)) {
    Write-Step 'Cleaning dist/'
    Remove-Item (Join-Path $Dist '*') -Recurse -Force -ErrorAction SilentlyContinue
}
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

$results = [ordered]@{}
foreach ($t in $want) {
    try {
        switch ($t) {
            'win'   { Build-Windows; $results['win']   = 'OK' }
            'linux' { Build-Linux;   $results['linux'] = 'OK' }
            'mac'   { Build-Macos;   $results['mac']   = 'OK (or skipped)' }
        }
    } catch {
        $results[$t] = "FAILED: $($_.Exception.Message)"
        Write-Warn2 "$t build failed: $($_.Exception.Message)"
    }
}

Write-Step 'Summary'
$results.GetEnumerator() | ForEach-Object { Write-Host ("  {0,-8} {1}" -f $_.Key, $_.Value) }
Write-Host "`nArtifacts in: $Dist" -ForegroundColor Cyan
Get-ChildItem $Dist -File -ErrorAction SilentlyContinue |
    ForEach-Object { Write-Host ("  - {0}" -f $_.Name) }

$failed = $results.Values | Where-Object { $_ -like 'FAILED*' }
if ($failed) { exit 1 }
