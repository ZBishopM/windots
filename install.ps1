#Requires -Version 7.0
<#
  Applies this rice to any Windows 11 machine: installs dependencies, deploys the
  configs (rewriting the hard-coded home path to yours), builds the Rust tools,
  wires up autostart, and applies the registry/env tweaks.

  Run from the repo root:   pwsh -ExecutionPolicy Bypass -File .\install.ps1
  Service tweaks need admin; the script will prompt (UAC) for that step only.
#>

$ErrorActionPreference = 'Stop'
$repo = $PSScriptRoot
$home_ = $env:USERPROFILE
$homeFwd = $home_ -replace '\\', '/'
$homeJson = $home_ -replace '\\', '\\'

function Say($m, $c = 'Cyan') { Write-Host "==> $m" -ForegroundColor $c }
function Ok($m) { Write-Host "    $m" -ForegroundColor DarkGray }

# Copy a text file to $dst, rewriting the original home path (all 3 forms) to yours.
function Deploy($src, $dst) {
    $c = Get-Content $src -Raw
    $c = $c.Replace('C:\\Users\\obisp', $homeJson).Replace('C:/Users/obisp', $homeFwd).Replace('C:\Users\obisp', $home_)
    $dir = Split-Path $dst
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
    Set-Content -Path $dst -Value $c -Encoding utf8 -NoNewline
    Ok "-> $dst"
}

function Shortcut($name, $target, $arguments = '') {
    $lnk = Join-Path ([Environment]::GetFolderPath('Startup')) "$name.lnk"
    $w = New-Object -ComObject WScript.Shell
    $s = $w.CreateShortcut($lnk); $s.TargetPath = $target
    if ($arguments) { $s.Arguments = $arguments }
    $s.Save(); Ok "autostart: $name"
}

# ---------------------------------------------------------------- 1. dependencies
Say '1/7  Dependencies'
if (-not (Get-Command scoop -EA SilentlyContinue)) {
    Ok 'installing scoop...'
    Invoke-RestMethod get.scoop.sh | Invoke-Expression
}
scoop bucket add main 2>$null; scoop bucket add extras 2>$null
foreach ($p in 'fastfetch', 'glazewm', 'altsnap', 'autohotkey', 'ffmpeg') {
    if (-not (scoop list $p 6>$null | Select-String $p)) { scoop install $p } else { Ok "have $p" }
}
if (-not (Get-Command wezterm -EA SilentlyContinue) -and -not (Test-Path "$env:ProgramFiles\WezTerm\wezterm.exe")) {
    winget install --id wez.wezterm --silent --accept-source-agreements --accept-package-agreements
}
if (-not (Get-Command cargo -EA SilentlyContinue)) {
    winget install --id Rustlang.Rustup --silent --accept-source-agreements --accept-package-agreements
    $env:Path += ";$home_\.cargo\bin"
}

# ---------------------------------------------------------------- 2. Rust tools
Say '2/7  Build Rust tools (glaze-bar, notify, loopback)'
$proj = "$home_\dev\glaze-bar"
New-Item -ItemType Directory -Force $proj | Out-Null
Copy-Item "$repo\glaze-bar\*" $proj -Recurse -Force
Push-Location $proj
cargo build --release
Pop-Location
Ok "built -> $proj\target\release\"

# WGC recorder (separate cargo project; windows-capture pulls a newer windows crate).
$wgc = "$home_\dev\shadowplay-wgc"
New-Item -ItemType Directory -Force $wgc | Out-Null
Copy-Item "$repo\shadowplay-wgc\*" $wgc -Recurse -Force
Push-Location $wgc
cargo build --release
Pop-Location
# The recorder spawns a sibling sysaudio-loopback.exe -> copy glaze-bar's build next to it.
Copy-Item "$proj\target\release\sysaudio-loopback.exe" "$wgc\target\release\sysaudio-loopback.exe" -Force
Ok "built WGC recorder -> $wgc\target\release\"

# ---------------------------------------------------------------- 3. configs
Say '3/7  Deploy configs'
Deploy "$repo\wezterm\.wezterm.lua"                       "$home_\.wezterm.lua"
Deploy "$repo\config\fastfetch\config.jsonc"              "$home_\.config\fastfetch\config.jsonc"
Deploy "$repo\config\fastfetch\duck.txt"                  "$home_\.config\fastfetch\duck.txt"
Deploy "$repo\config\glazewm\config.yaml"                 "$home_\.glzr\glazewm\config.yaml"
Deploy "$repo\powershell\Microsoft.PowerShell_profile.ps1" "$home_\Documents\PowerShell\Microsoft.PowerShell_profile.ps1"
foreach ($f in 'glazewm-dwindle.ps1', 'wezterm-hotkey.ahk', 'shadowplay-record.ps1', 'shadowplay-record.vbs', 'shadowplay-save.ps1', 'shadowplay-wgc-save.ps1', 'shadowplay-wgc.vbs', 'rice-supervisor.ps1', 'rice-supervisor.vbs', 'rice-autostart.ps1', 'rice-autostart.vbs') {
    Deploy "$repo\scripts\$f" "$home_\.config\$f"
}
# AltSnap.ini is UTF-16 and has no paths -> copy raw, into scoop persist.
$asPersist = "$home_\scoop\persist\altsnap"
if (Test-Path (Split-Path $asPersist)) {
    New-Item -ItemType Directory -Force $asPersist | Out-Null
    Copy-Item "$repo\altsnap\AltSnap.ini" "$asPersist\AltSnap.ini" -Force
    Copy-Item "$repo\altsnap\AltSnap.ini" "$home_\scoop\apps\altsnap\current\AltSnap.ini" -Force -EA SilentlyContinue
    Ok 'AltSnap.ini (Win modifier)'
}

# ---------------------------------------------------------------- 4. folders
Say '4/7  ShadowPlay folders'
New-Item -ItemType Directory -Force "$home_\ShadowPlay\buffer", "$home_\ShadowPlay\wgc-buffer", "$home_\ShadowPlay\clips" | Out-Null

# ---------------------------------------------------------------- 5. autostart
Say '5/7  Autostart'
$scoopApps = "$home_\scoop\apps"
Shortcut 'GlazeWM'         "$scoopApps\glazewm\current\GlazeWM.exe"
Shortcut 'AltSnap'         "$scoopApps\altsnap\current\AltSnap.exe"
Shortcut 'wezterm-hotkey'  "$scoopApps\autohotkey\current\v2\AutoHotkey64.exe" "`"$home_\.config\wezterm-hotkey.ahk`""
Shortcut 'ShadowPlay'      'wscript.exe' "`"$home_\.config\shadowplay-wgc.vbs`""  # WGC recorder (ddagrab shadowplay-record.vbs kept for the v1.0 fallback)
# Supervisor: relaunches any of the above that dies (crash, kill, GlazeWM restart).
Shortcut 'RiceSupervisor'  'wscript.exe' "`"$home_\.config\rice-supervisor.vbs`""
# Autostart: opens the working app/workspace layout once at login. App paths + the
# Chrome profile + Claude AUMID inside are machine-specific; edit for your setup.
Shortcut 'RiceAutostart'   'wscript.exe' "`"$home_\.config\rice-autostart.vbs`""

# ---------------------------------------------------------------- 6. registry / env
Say '6/7  Registry + env tweaks'
$tg = 'HKCU:\Keyboard Layout\Toggle'
if (-not (Test-Path $tg)) { New-Item -Path $tg -Force | Out-Null }
'Language Hotkey', 'Hotkey', 'Layout Hotkey' | ForEach-Object { Set-ItemProperty $tg -Name $_ -Value '3' -Type String }
Ok 'Alt+Shift language switch disabled (use Win+Space for lang / Command Palette)'
[Environment]::SetEnvironmentVariable('POWERSHELL_TELEMETRY_OPTOUT', '1', 'User')
[Environment]::SetEnvironmentVariable('DOTNET_CLI_TELEMETRY_OPTOUT', '1', 'User')
Ok 'telemetry optout'

# ---------------------------------------------------------------- 7. system tweaks (admin)
Say '7/7  Disable unused services + MPO (optional, needs admin)'
$svc = 'DiagTrack', 'SysMain', 'DPS', 'Spooler'
$adminCmd = ($svc | ForEach-Object { "Set-Service $_ -StartupType Disabled -EA SilentlyContinue; Stop-Service $_ -Force -EA SilentlyContinue" }) -join '; '
# Disable Multi-Plane Overlay: hardware video overlays (MPO) bypass DWM
# composition, so Desktop Duplication (ddagrab) can't see them and ShadowPlay
# captures a FROZEN frame whenever a hardware-accelerated video plays. Forcing
# DWM to composite everything fixes it. Takes effect after a reboot.
$adminCmd += '; reg add "HKLM\SOFTWARE\Microsoft\Windows\Dwm" /v OverlayTestMode /t REG_DWORD /d 5 /f'
try {
    Start-Process pwsh -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile', '-Command', $adminCmd
    Ok 'services disabled + MPO off (reboot to apply MPO)'
} catch { Ok 'skipped (no elevation) - see README to do it manually' }

Write-Host ''
Say 'Done. Log out/in (or reboot) to start everything.' Green
Write-Host @"
    Manual bits (see README):
      - Command Palette on Win+Space: install PowerToys, then set its hotkey.
      - Monitor layout: glaze-bar --x/--width in glazewm config.yaml startup_commands
        and the notify position are hard-coded to 1920 + 2560; adjust for your screens.
"@ -ForegroundColor DarkGray
