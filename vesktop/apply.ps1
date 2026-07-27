#Requires -Version 7.0
<#
  Patches Vesktop so Discord's notifications are drawn by shadowplay-notify
  instead of the stock blue Windows toast.

    pwsh -File .\apply.ps1            # (re-)apply
    pwsh -File .\apply.ps1 -Check     # report only, exit 1 if not applied
    pwsh -File .\apply.ps1 -Revert    # strip the patch, leave Vesktop stock

  There is no supported extension point for this. Vesktop ships Vencord as four
  prebuilt files it downloads into its own data directory, and the notification
  path lives entirely in the renderer, so the patch is an append to three of
  those files (see notify-*.js for which and why). The appends are delimited by
  >>> / <<< rice-notify markers, which is what makes this idempotent and what
  -Revert cuts back out.

  What this deliberately does NOT touch: resources\app.asar. Patching that would
  need a repack of a 30 MB archive and would be destroyed by every scoop update
  of Vesktop, with no way to notice. The files patched here live under
  scoop\persist, so a Vesktop update leaves them alone -- it is a *Vencord*
  update that wipes them, and notify-main.js re-applies itself when that happens.

  Nothing here needs elevation, and nothing here restarts Vesktop: the patch is
  read at launch, so an already-running Vesktop keeps its stock toasts until you
  restart it yourself.
#>
param([switch]$Check, [switch]$Revert)

$ErrorActionPreference = 'Stop'
$home_ = $env:USERPROFILE
$stage = "$home_\.config\vesktop"

$Begin = '// >>> rice-notify'
$Fragments = [ordered]@{
    'vencordDesktopMain.js'     = 'notify-main.js'
    'vencordDesktopPreload.js'  = 'notify-preload.js'
    'vencordDesktopRenderer.js' = 'notify-renderer.js'
}

function Say($m, $c = 'Cyan') { Write-Host "==> $m" -ForegroundColor $c }
function Ok($m) { Write-Host "    $m" -ForegroundColor DarkGray }

# UTF-8 without BOM, no line-ending translation. Set-Content would be fine for
# the two small files but the renderer bundle is 700 KB of minified JS that is
# eval'd verbatim; there is no reason to let PowerShell near its bytes.
$Utf8 = [Text.UTF8Encoding]::new($false)
function ReadText($p) { [IO.File]::ReadAllText($p, $Utf8) }
function WriteText($p, $t) { [IO.File]::WriteAllText($p, $t, $Utf8) }

# ------------------------------------------------------------------- 1. stage
# The fragments have to exist somewhere stable and outside Vesktop's data
# directory: notify-main.js reads them back from $stage to re-apply itself after
# a Vencord update. Same home-path rewrite install.ps1's Deploy() does, so this
# works when run straight from the repo on another machine.
#
# Skipped under -Check so that sync.ps1 -Check, which shells out to this, really
# does change nothing, and under -Revert because putting files back is not a
# reason to freshen them.
if (-not $Check -and -not $Revert -and $PSScriptRoot -ne $stage) {
    $homeFwd = $home_ -replace '\\', '/'
    $homeJson = $home_ -replace '\\', '\\'
    New-Item -ItemType Directory -Force $stage | Out-Null
    foreach ($f in @($Fragments.Values) + @('apply.ps1')) {
        $c = ReadText (Join-Path $PSScriptRoot $f)
        $c = $c.Replace('C:\\Users\\obisp', $homeJson).Replace('C:/Users/obisp', $homeFwd).Replace('C:\Users\obisp', $home_)
        WriteText (Join-Path $stage $f) $c
    }
    Ok "staged fragments -> $stage"
}

# ------------------------------------------------------------------- 2. locate
# Mirrors Vesktop's own resolution (dist/js/main.js): VENCORD_USER_DATA_DIR wins,
# otherwise an install with no "Uninstall Vesktop.exe" beside the exe counts as
# portable and keeps its Data next to the binary -- which is what scoop produces,
# with Data symlinked into scoop\persist.
$dataDir = $env:VENCORD_USER_DATA_DIR
if (-not $dataDir) {
    $dataDir = @("$home_\scoop\apps\vesktop\current\Data", "$env:APPDATA\vesktop") |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $dataDir) {
    Ok 'no Vesktop data directory found - skipping'
    exit 0
}

# A custom Vencord directory (Vesktop settings -> "Vencord Location") overrides
# the downloaded one, and Vencord's updater writes to whichever is in use.
$vencordDir = $null
$state = Join-Path $dataDir 'state.json'
if (Test-Path $state) { $vencordDir = (Get-Content $state -Raw | ConvertFrom-Json).vencordDir }
if (-not $vencordDir) { $vencordDir = Join-Path $dataDir 'sessionData\vencordFiles' }

if (-not (Test-Path $vencordDir)) {
    Ok "Vencord not downloaded yet ($vencordDir) - launch Vesktop once, then re-run"
    exit 0
}
Say "Vesktop notifications -> shadowplay-notify"
Ok "vencord dir: $vencordDir"

# ------------------------------------------------------------------- 3. patch
$missing = @()
foreach ($target in $Fragments.Keys) {
    $dst = Join-Path $vencordDir $target
    if (-not (Test-Path $dst)) { $missing += "$target (absent)"; continue }

    $text = ReadText $dst
    # Everything from the first marker on is ours -- the block is only ever
    # appended -- so cutting there is enough and avoids a regex over 700 KB.
    $i = $text.IndexOf($Begin)
    $applied = $i -ge 0

    if ($Check) {
        if ($applied) { Ok "patched   $target" } else { $missing += $target }
        continue
    }

    if ($applied) { $text = $text.Substring(0, $i) }
    $text = $text.TrimEnd("`r", "`n") + "`n"
    if (-not $Revert) { $text += ReadText (Join-Path $stage $Fragments[$target]) }
    WriteText $dst $text
    Ok "$(if ($Revert) { 'reverted ' } else { 'patched  ' }) $target"
}

if ($Check) {
    if ($missing) {
        Write-Host "vesktop notification patch NOT applied ($($missing.Count)):" -ForegroundColor Yellow
        $missing | ForEach-Object { Write-Host "  $_" }
        exit 1
    }
    Write-Host 'vesktop notification patch is applied' -ForegroundColor Green
    exit 0
}
if ($missing) { Ok "skipped (not in this Vencord build): $($missing -join ', ')" }

# The renderer script is read at window creation, so a running client is still on
# the old copy. Restarting it is the user's call -- this script never kills apps.
if (Get-Process -Name vesktop -ErrorAction SilentlyContinue) {
    Write-Host '    Vesktop is running - restart it (tray -> Restart) to pick this up.' -ForegroundColor Yellow
}
if (-not $Revert -and -not (Test-Path "$home_\dev\target\release\shadowplay-notify.exe")) {
    Write-Host '    shadowplay-notify.exe is missing - build the workspace or toasts stay stock.' -ForegroundColor Yellow
}
