#Requires -Version 7.0
<#
  Copies the LIVE rice back into this repo -- the reverse of install.ps1.

  install.ps1 only ever goes repo -> live, so the repo drifts silently every time
  a config is edited in place. Nothing detected that; it stayed in sync by
  remembering to copy files by hand.

    pwsh -File .\sync.ps1            # pull live -> repo
    pwsh -File .\sync.ps1 -Check     # report differences, change nothing,
                                     # exit 1 if any (usable as a pre-commit hook)
#>
param([switch]$Check)

$ErrorActionPreference = 'Stop'
$repo = $PSScriptRoot
$home_ = $env:USERPROFILE

# repo-relative path  ->  live path. The single source of truth for both
# directions; install.ps1's deploy list mirrors it.
$Map = [ordered]@{
    'wezterm\.wezterm.lua'                       = "$home_\.wezterm.lua"
    'config\glazewm\config.yaml'                 = "$home_\.glzr\glazewm\config.yaml"
    'config\fastfetch\config.jsonc'              = "$home_\.config\fastfetch\config.jsonc"
    'config\fastfetch\duck.txt'                  = "$home_\.config\fastfetch\duck.txt"
    'powershell\Microsoft.PowerShell_profile.ps1' = "$home_\Documents\PowerShell\Microsoft.PowerShell_profile.ps1"
    'nushell\config.nu'                          = "$home_\AppData\Roaming\nushell\config.nu"
    'altsnap\AltSnap.ini'                        = "$home_\scoop\persist\altsnap\AltSnap.ini"
}
foreach ($f in 'glazewm-dwindle.ps1', 'glazewm-animcheck.ps1', 'wezterm-hotkey.ahk',
               'shadowplay-record.ps1', 'shadowplay-record.vbs', 'shadowplay-wgc-save.ps1',
               'shadowplay-wgc.vbs', 'rice-supervisor.ps1', 'rice-supervisor.vbs',
               'rice-autostart.ps1', 'rice-autostart.vbs') {
    $Map["scripts\$f"] = "$home_\.config\$f"
}
foreach ($f in Get-ChildItem "$home_\.config\lib\*.ps1" -EA SilentlyContinue) {
    $Map["scripts\lib\$($f.Name)"] = $f.FullName
}

# Rust sources: whole trees, minus build output.
$Trees = @{ 'crates' = "$home_\dev\crates" }
$Loose = @{ 'Cargo.toml' = "$home_\dev\Cargo.toml"; 'Cargo.lock' = "$home_\dev\Cargo.lock" }

$diff = @()
$copied = 0

function Sync-One($rel, $live) {
    $dst = Join-Path $repo $rel
    if (-not (Test-Path $live)) { $script:diff += "MISSING LIVE  $rel"; return }
    $same = (Test-Path $dst) -and -not (Compare-Object (Get-Content $live -Raw -EA SilentlyContinue) (Get-Content $dst -Raw -EA SilentlyContinue))
    if ($same) { return }
    $script:diff += "DIFFERS       $rel"
    if (-not $Check) {
        $dir = Split-Path $dst
        if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
        Copy-Item $live $dst -Force
        $script:copied++
    }
}

foreach ($k in $Map.Keys) { Sync-One $k $Map[$k] }
foreach ($k in $Loose.Keys) { Sync-One $k $Loose[$k] }

foreach ($rel in $Trees.Keys) {
    $live = $Trees[$rel]
    if (-not (Test-Path $live)) { continue }
    foreach ($f in Get-ChildItem $live -Recurse -File | Where-Object { $_.FullName -notmatch '\\target\\' }) {
        $sub = $f.FullName.Substring($live.Length).TrimStart('\')
        Sync-One (Join-Path $rel $sub) $f.FullName
    }
}

if ($Check) {
    if ($diff.Count) {
        Write-Host "repo is OUT OF SYNC with live ($($diff.Count)):" -ForegroundColor Yellow
        $diff | ForEach-Object { Write-Host "  $_" }
        exit 1
    }
    Write-Host 'repo matches live' -ForegroundColor Green
    exit 0
}

if ($copied) {
    Write-Host "pulled $copied file(s) from live:" -ForegroundColor Cyan
    $diff | ForEach-Object { Write-Host "  $_" }
} else {
    Write-Host 'already in sync' -ForegroundColor Green
}
