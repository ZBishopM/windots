#Requires -Version 7.0
<#
  Applies this rice to any Windows 11 machine: installs dependencies, deploys the
  configs (rewriting the hard-coded home path to yours), builds the Rust tools,
  wires up autostart, and applies the registry/env tweaks.

  Run from the repo root:   pwsh -ExecutionPolicy Bypass -File .\install.ps1
  Service tweaks need admin; the script will prompt (UAC) for that step only.
#>

$ErrorActionPreference = 'Stop'
# Without this, a *native* command that exits non-zero (cargo, scoop, winget) does
# NOT trip $ErrorActionPreference in PowerShell 7 -- the install would sail past a
# failed `cargo build`, report success, and then wire up Startup shortcuts pointing
# at .exe files that were never produced.
$PSNativeCommandUseErrorActionPreference = $true
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
Say '1/8  Dependencies'
if (-not (Get-Command scoop -EA SilentlyContinue)) {
    Ok 'installing scoop...'
    Invoke-RestMethod get.scoop.sh | Invoke-Expression
}
scoop bucket add main 2>$null; scoop bucket add extras 2>$null
# nerd-fonts: the bar, the toasts and WezTerm all render Nerd Font glyphs and all
# three fail *silently* to tofu boxes when the font is missing.
scoop bucket add nerd-fonts 2>$null
# btop is not started as an app: rice-autostart opens it INSIDE wezterm
# (`wezterm start -- pwsh -NoExit -Command btop`), so what it needs is to be on
# PATH. JetBrainsMono-NF is the font every UI component hardcodes.
foreach ($p in 'fastfetch', 'glazewm', 'altsnap', 'autohotkey', 'ffmpeg', 'nu', 'btop', 'JetBrainsMono-NF') {
    # Match the package name exactly: `scoop list nu | Select-String nu` matches any
    # installed package containing "nu", which silently skipped nushell.
    if (scoop list $p 6>$null | Where-Object Name -eq $p) { Ok "have $p" } else { scoop install $p }
}
if (-not (Get-Command wezterm -EA SilentlyContinue) -and -not (Test-Path "$env:ProgramFiles\WezTerm\wezterm.exe")) {
    winget install --id wez.wezterm --silent --accept-source-agreements --accept-package-agreements
}
# No PowerToys, deliberately. The Win+Space search box is crates/launcher: built
# in step 3, kept resident by rice-supervisor, invoked by wezterm-hotkey.ahk with
# `--show`. PowerToys' Command Palette did that same job for 267 MB resident and
# 59 s of cold start -- measured, and the largest single item in the post-login
# budget. Installing it here would put both back.
if (-not (Get-Command cargo -EA SilentlyContinue)) {
    winget install --id Rustlang.Rustup --silent --accept-source-agreements --accept-package-agreements
    $env:Path += ";$home_\.cargo\bin"
}

# ---------------------------------------------------------------- 2. configs
# Configs are deployed BEFORE the Rust build on purpose: the build is the step
# most likely to fail on a fresh machine (missing/misresolved toolchain), and a
# failure there used to abort the installer before a single config was written.
Say '2/8  Deploy configs'
# rice.json PRIMERO, y no es un detalle de orden: es la unica configuracion del
# escritorio y hasta ahora este instalador NUNCA la desplegaba. Estaba solo en el
# mapa de sync.ps1, que va en la direccion contraria (vivo -> repo). En una
# maquina limpia no habia ~\.config\rice.json, asi que corrian los valores por
# defecto de Rust en vez de estos, no habia nada que editar en caliente, y el
# esquema tampoco llegaba -- o sea que el editor no autocompletaba ni validaba.
Deploy "$repo\config\rice.json"                           "$home_\.config\rice.json"
Deploy "$repo\config\rice.schema.json"                    "$home_\.config\rice.schema.json"
Deploy "$repo\wezterm\.wezterm.lua"                       "$home_\.wezterm.lua"
Deploy "$repo\config\fastfetch\config.jsonc"              "$home_\.config\fastfetch\config.jsonc"
Deploy "$repo\config\fastfetch\duck.txt"                  "$home_\.config\fastfetch\duck.txt"
Deploy "$repo\config\glazewm\config.yaml"                 "$home_\.glzr\glazewm\config.yaml"
Deploy "$repo\powershell\Microsoft.PowerShell_profile.ps1" "$home_\Documents\PowerShell\Microsoft.PowerShell_profile.ps1"
# Nushell: the fast interactive shell (pwsh still runs the infra scripts). Config
# carries fastfetch + the rice tool wrappers (cava/mic/notify-test).
Deploy "$repo\nushell\config.nu"                          "$home_\AppData\Roaming\nushell\config.nu"
# rice-llm.ps1 and rice-uninstall.ps1 are not optional extras: crates/launcher
# hard-codes %USERPROFILE%\.config\ paths to them (see commands.rs), so a machine
# where they were never deployed gets Win+Space entries that resolve to nothing
# and fail only when someone picks them.
#
# The one-shot tuning scripts go out too, even though nothing launches them. A
# fresh machine is exactly where they are wanted: trim-background and trim-run
# turn off the resident software this desktop replaced, retire-replaced
# uninstalls it, boot-report says what login cost. Leaving them in the repo
# only meant re-deriving them by hand on the next machine.
foreach ($f in 'glazewm-dwindle.ps1', 'glazewm-animcheck.ps1', 'rice-accent.ps1', 'wezterm-hotkey.ahk', 'shadowplay-record.ps1', 'shadowplay-record.vbs', 'shadowplay-wgc-save.ps1', 'shadowplay-verificar.ps1', 'shadowplay-wgc.vbs', 'rice-supervisor.ps1', 'rice-supervisor.vbs', 'rice-autostart.ps1', 'rice-autostart.vbs', 'rice-llm.ps1', 'rice-uninstall.ps1',
               'rice-trim-background.ps1', 'rice-trim-run.ps1', 'rice-tame-startup.ps1', 'rice-retire-replaced.ps1', 'rice-boot-report.ps1', 'rice-notif-banners.ps1', 'rice-tray-promote.ps1',
               'rice-clip-share.ps1', 'rice-onedrive-purge.ps1', 'rice-deshacer.ps1', 'rice-teclado.ps1') {
    Deploy "$repo\scripts\$f" "$home_\.config\$f"
}
# Shared library dot-sourced by the scripts above (paths, GlazeWM IPC, process
# helpers). Every one of them fails at its first line without these.
foreach ($f in Get-ChildItem "$repo\scripts\lib\*.ps1") {
    Deploy $f.FullName "$home_\.config\lib\$($f.Name)"
}
# Plantilla de secretos, VACIA y solo si no existe.
#
# Este archivo no se versiona -- con la URL del webhook cualquiera publica en tu
# canal y el repo es publico -- asi que en una maquina limpia sencillamente NO
# ESTA, y hasta ahora nada lo decia. El resultado era un fallo mudo perfecto:
# Alt+F10 graba, sube a catbox, y no avisa nunca a Discord sin que nada explique
# por que. Peor: su documentacion vivia DENTRO del propio archivo, que es
# justamente el que no tienes.
#
# Se crea vacio, nunca se pisa uno existente, y lleva las instrucciones dentro.
$secrets = "$home_\.config\rice-secrets.json"
if (Test-Path $secrets) {
    Ok 'rice-secrets.json ya existe - no se toca'
} else {
    @'
{
  "_nota": "Fuera del repo a proposito: con la URL del webhook cualquiera puede publicar en tu canal, y dotfiles es publico. No esta en el mapa de sync.ps1 y esta en .gitignore.",
  "_como": "Discord > Ajustes del servidor > Integraciones > Webhooks > Nuevo webhook > Copiar URL",
  "discord_webhook": "",
  "_catbox": "Opcional pero recomendado: sin userhash las subidas son anonimas y NO SE PUEDEN BORRAR (la API devuelve 412). Saca el tuyo en catbox.moe > Manage account > User hash.",
  "catbox_userhash": ""
}
'@ | Set-Content -Path $secrets -Encoding utf8
    Ok "plantilla creada: $secrets  (rellena discord_webhook para que avise a Discord)"
}
# Firefox AutoConfig -- the only two files in this repo that do NOT live under
# $home_. AutoConfig is read from the application directory next to firefox.exe,
# which is how it gets to override @mozilla.org/alerts-service;1 before any
# profile JS exists. Program Files needs elevation, so they are only *staged*
# here (Deploy still rewrites the home path); step 8 copies them in under the one
# UAC prompt the installer already asks for.
$ffStage = "$home_\.config\firefox-autoconfig"
Deploy "$repo\firefox\config.js"                     "$ffStage\config.js"
Deploy "$repo\firefox\defaults\pref\config-prefs.js" "$ffStage\config-prefs.js"
$ffDirs = @("$env:ProgramFiles\Firefox Developer Edition", "$env:ProgramFiles\Mozilla Firefox") |
    Where-Object { Test-Path $_ }
if (-not $ffDirs) { Ok 'no Firefox in Program Files - notification override will be skipped' }

# No per-app notification patch is installed, for Discord or anything else.
# notifyd subscribes to the system notification listener, so it sees and redraws
# every app's notifications -- including the Discord client's -- from one single
# process. dotfiles\vesktop\ is the pre-notifyd approach kept for reference: an
# append to three of Vencord's prebuilt bundle files, because Discord raises its
# notifications from the renderer's window.Notification and Vesktop offered no
# plugin hook. It was per-app by nature, so it bought one client and every other
# app kept the stock blue toast; that is the reason not to reach for it again.

# ---------------------------------------------------------------- 3. Rust tools
# One cargo workspace (dev/Cargo.toml) builds every bin -- glaze-bar, taskbar,
# launcher, notifyd, cava, sysaudio-loopback, shadowplay-notify, shadowplay-wgc,
# micswitch, appvol, winkill, ws-slide -- into a single dev/target/release/. The
# recorder and cava find their sibling sysaudio-loopback.exe there automatically,
# so there is no copy step. rice-common is the shared library and yields no exe.
Say '3/8  Build Rust tools (cargo workspace)'
$dev = "$home_\dev"
New-Item -ItemType Directory -Force "$dev\crates" | Out-Null
Copy-Item "$repo\crates\*"   "$dev\crates"      -Recurse -Force
Copy-Item "$repo\Cargo.toml" "$dev\Cargo.toml"  -Force   # workspace root
Copy-Item "$repo\Cargo.lock" "$dev\Cargo.lock"  -Force
Push-Location $dev
try { cargo build --release } finally { Pop-Location }
Ok "built workspace -> $dev\target\release\"
# AltSnap.ini is UTF-16 and has no paths -> copy raw, into scoop persist.
$asPersist = "$home_\scoop\persist\altsnap"
if (Test-Path (Split-Path $asPersist)) {
    New-Item -ItemType Directory -Force $asPersist | Out-Null
    Copy-Item "$repo\altsnap\AltSnap.ini" "$asPersist\AltSnap.ini" -Force
    Copy-Item "$repo\altsnap\AltSnap.ini" "$home_\scoop\apps\altsnap\current\AltSnap.ini" -Force -EA SilentlyContinue
    Ok 'AltSnap.ini (Win modifier)'
}

# ---------------------------------------------------------------- 4. folders
Say '4/8  ShadowPlay folders'
New-Item -ItemType Directory -Force "$home_\ShadowPlay\buffer", "$home_\ShadowPlay\wgc-buffer", "$home_\ShadowPlay\clips" | Out-Null

# ---------------------------------------------------------------- 5. autostart
Say '5/8  Autostart'
$scoopApps = "$home_\scoop\apps"
Shortcut 'GlazeWM'         "$scoopApps\glazewm\current\GlazeWM.exe"
Shortcut 'AltSnap'         "$scoopApps\altsnap\current\AltSnap.exe"
Shortcut 'wezterm-hotkey'  "$scoopApps\autohotkey\current\v2\AutoHotkey64.exe" "`"$home_\.config\wezterm-hotkey.ahk`""
Shortcut 'ShadowPlay'      'wscript.exe' "`"$home_\.config\shadowplay-wgc.vbs`""  # WGC recorder (ddagrab shadowplay-record.vbs kept for the v1.0 fallback)
# Supervisor: relaunches any of the above that dies (crash, kill, GlazeWM restart).
Shortcut 'RiceSupervisor'  'wscript.exe' "`"$home_\.config\rice-supervisor.vbs`""
# Autostart: opens the working app/workspace layout once at login. The app paths
# inside (WezTerm, Zed, Firefox Developer Edition) and the Claude AUMID are
# machine-specific; edit for your setup.
Shortcut 'RiceAutostart'   'wscript.exe' "`"$home_\.config\rice-autostart.vbs`""

# ---------------------------------------------------------------- 6. registry / env
Say '6/8  Registry + env tweaks'
$tg = 'HKCU:\Keyboard Layout\Toggle'
if (-not (Test-Path $tg)) { New-Item -Path $tg -Force | Out-Null }
# Value 3 is "none". Alt+Shift fires by accident far too easily: one stray press
# stepped the input language on to the Chinese IME and every keystroke after that
# came out as pinyin, with no obvious way back. wezterm-hotkey.ahk puts the
# deliberate replacement on Ctrl+Alt+Shift+Space -- Win+Space belongs to the
# launcher, and plain Ctrl+Alt+Space IS AltGr+Space on a Spanish layout.
'Language Hotkey', 'Hotkey', 'Layout Hotkey' | ForEach-Object { Set-ItemProperty $tg -Name $_ -Value '3' -Type String }
Ok 'Alt+Shift language switch disabled (Ctrl+Alt+Shift+Space cycles it instead)'
[Environment]::SetEnvironmentVariable('POWERSHELL_TELEMETRY_OPTOUT', '1', 'User')
[Environment]::SetEnvironmentVariable('DOTNET_CLI_TELEMETRY_OPTOUT', '1', 'User')
Ok 'telemetry optout'

# ---------------------------------------------------------------- 7. notifyd package
# notifyd redraws every Windows notification with the rice's toast, and to
# subscribe to the notification listener's change event it needs *package
# identity*. That comes from a sparse MSIX registered against the folder cargo
# already built into -- nothing moves, no binary is copied.
#
# Staged exactly like the Firefox AutoConfig above and for the same reason: the
# build and the signing need no admin and happen here, the one elevated part
# (trusting the self-signed certificate) is folded into step 8's single UAC
# prompt, and the registration -- which is per-user and needs no admin -- runs
# right after it. Non-fatal throughout: without the package notifyd still works
# by polling, just a poll interval slower.
Say '7/8  notifyd sparse package'
$npStage = "$home_\.config\notifyd-package"
Deploy "$repo\notifyd-package\AppxManifest.xml" "$npStage\AppxManifest.xml"
Deploy "$repo\notifyd-package\build.ps1"        "$npStage\build.ps1"
$npCer = "$npStage\notifyd-sparse.cer"
$npOk = $false
try {
    & "$npStage\build.ps1" -ExternalLocation "$dev\target\release" -OutDir $npStage
    $npOk = Test-Path $npCer
} catch { Ok "notifyd package build skipped: $_" }

# ---------------------------------------------------------------- 8. system tweaks (admin)
Say '8/8  Firefox AutoConfig + notifyd cert + disable unused services + MPO (optional, needs admin)'
# DPS is in this list knowingly, and it is the one with a price. It is the service
# that writes the Diagnostics-Performance events, which is where
# scripts/rice-boot-report.ps1 gets its numbers -- disabled here, that report has
# nothing new to show and says so. The trade taken: boot timings are wanted for a
# few days while tuning startup, the service runs at every boot forever. To
# measure again, elevated:
#     Set-Service DPS -StartupType Manual; Start-Service DPS
# then reboot; the event lands a couple of minutes after login.
$svc = 'DiagTrack', 'SysMain', 'DPS', 'Spooler'
$adminCmd = ($svc | ForEach-Object { "Set-Service $_ -StartupType Disabled -EA SilentlyContinue; Stop-Service $_ -Force -EA SilentlyContinue" }) -join '; '
# The staged AutoConfig pair, folded into the same elevated call so the installer
# still prompts for UAC exactly once. Firefox has to be restarted to pick it up.
foreach ($d in $ffDirs) {
    $adminCmd += "; New-Item -ItemType Directory -Force '$d\defaults\pref' | Out-Null"
    $adminCmd += "; Copy-Item '$ffStage\config.js' '$d\config.js' -Force"
    $adminCmd += "; Copy-Item '$ffStage\config-prefs.js' '$d\defaults\pref\config-prefs.js' -Force"
}
# Disable Multi-Plane Overlay: hardware video overlays (MPO) bypass DWM
# composition, so Desktop Duplication (ddagrab) can't see them and ShadowPlay
# captures a FROZEN frame whenever a hardware-accelerated video plays. Forcing
# DWM to composite everything fixes it. Takes effect after a reboot.
$adminCmd += '; reg add "HKLM\SOFTWARE\Microsoft\Windows\Dwm" /v OverlayTestMode /t REG_DWORD /d 5 /f'
# The sparse package is self-signed, so Windows will not register it until its
# certificate is trusted machine-wide. TrustedPeople, NOT Root: that is the store
# MSIX deployment actually consults, and it grants far less than Root does.
if ($npOk) {
    $adminCmd += "; Import-Certificate -FilePath '$npCer' -CertStoreLocation Cert:\LocalMachine\TrustedPeople | Out-Null"
}
$elevated = $false
try {
    Start-Process pwsh -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile', '-Command', $adminCmd
    $elevated = $true
    Ok 'services disabled + MPO off (reboot to apply MPO)'
    if ($ffDirs) { Ok 'Firefox AutoConfig installed (restart Firefox)' }
    if ($npOk) { Ok 'notifyd signing certificate trusted' }
} catch {
    Ok 'skipped (no elevation) - see README to do it manually'
    if ($ffDirs) { Ok "  Firefox part, run elevated:  $adminCmd" }
}

# Registration is per-user and needs no admin, but it does need the certificate
# from the step above, so it can only run once that has actually happened.
if ($npOk -and $elevated) {
    try {
        & "$npStage\build.ps1" -ExternalLocation "$dev\target\release" -OutDir $npStage -Register
        Ok 'notifyd sparse package registered'
    } catch { Ok "notifyd registration skipped: $_" }
}

Write-Host ''
Say 'Done. Log out/in (or reboot) to start everything.' Green
Write-Host @"
    Manual bits (see README):
      - Monitor layout: glaze-bar --x/--width in glazewm config.yaml startup_commands
        and the notify position are hard-coded to 1920 + 2560; adjust for your screens.
      - Turn Do Not Disturb ON (Settings > System > Notifications, or Win+N and the
        bell). This is what suppresses the stock blue banners; notifyd still receives
        every notification and redraws it. Not done automatically: it is a visible
        system-wide setting and it is yours to choose.
        Note the trade: with DND on and notifyd dead, notifications appear nowhere
        but the Notification Center. rice-supervisor restarts it within 30s.
      - Check it:  ~\dev\target\release\notifyd.exe --check
                   then  cat ~\.config\logs\notifyd.log
"@ -ForegroundColor DarkGray

