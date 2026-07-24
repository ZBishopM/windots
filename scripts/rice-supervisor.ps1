# Keeps the rice's always-on processes alive. Every 30s it checks each component
# and relaunches any that died (crash, manual kill, GlazeWM restart killing its
# child dwindle, etc.) so a dead piece self-heals within a minute instead of
# staying dead until the next login. Single-instance; trims its own RAM.

$mutex = New-Object System.Threading.Mutex($false, 'Global\rice-supervisor')
if (-not $mutex.WaitOne(0)) { exit }   # another supervisor already running

$cfg    = 'C:\Users\obisp\.config'
$scoop  = 'C:\Users\obisp\scoop\apps'
$ahk    = "$scoop\autohotkey\current\v2\AutoHotkey64.exe"
$bar    = 'C:\Users\obisp\dev\target\release\glaze-bar.exe'
$wsslide = 'C:\Users\obisp\dev\target\release\ws-slide.exe'

# Give things a moment at login so we don't race the normal autostart.
Start-Sleep -Seconds 20

Add-Type -Namespace W -Name K -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("psapi.dll")] public static extern bool EmptyWorkingSet(System.IntPtr h);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern System.IntPtr GetCurrentProcess();
'@ -EA SilentlyContinue

function Alive($name)      { [bool](Get-Process $name -EA SilentlyContinue) }
function AliveCount($name) { (Get-Process $name -EA SilentlyContinue | Measure-Object).Count }
function AliveCmd($match)  { [bool](Get-CimInstance Win32_Process -Filter "Name='pwsh.exe'" -EA SilentlyContinue | Where-Object { $_.CommandLine -like $match }) }

# GlazeWM can wedge: the process stays alive but its IPC (and keybinds) stop
# responding, which a plain process check misses. Probe the IPC to tell healthy
# from hung.
# IMPORTANT: connect to 127.0.0.1, NOT 'localhost'. 'localhost' resolves ::1
# (IPv6) first, which GlazeWM's IPC doesn't listen on, so the connect eats the
# full ~2.1s IPv6 timeout before falling back to IPv4 -- that overshot the old
# 2000ms wait, so the probe returned false on a perfectly healthy WM and the
# supervisor killed + restarted GlazeWM every ~60s (which re-ran dwindle and
# flashed a Windows Terminal window). 127.0.0.1 connects in ~5ms.
function GlazeHealthy {
    if (-not (Alive 'GlazeWM')) { return $false }
    try {
        $t = New-Object System.Net.WebSockets.ClientWebSocket
        $k = [System.Threading.CancellationToken]::None
        if (-not $t.ConnectAsync([Uri]'ws://127.0.0.1:6123', $k).Wait(4000)) { $t.Dispose(); return $false }
        $mm = [Text.Encoding]::UTF8.GetBytes('query windows')
        [void]$t.SendAsync((New-Object System.ArraySegment[byte] (, $mm)), 'Text', $true, $k).Wait(2000)
        $ok = $t.ReceiveAsync((New-Object System.ArraySegment[byte] (, (New-Object byte[] 2048))), $k).Wait(2000)
        $t.Dispose()
        return [bool]$ok
    } catch { return $false }
}

$glzFails = 0
while ($true) {
    # Restart GlazeWM if it's gone (now) or hung (IPC dead for ~3 checks, to avoid
    # nuking it on a transient hiccup -- a real wedge stays dead, a hiccup recovers).
    if (-not (Alive 'GlazeWM')) {
        Start-Process "$scoop\glazewm\current\GlazeWM.exe"; $glzFails = 0
    }
    elseif (GlazeHealthy) { $glzFails = 0 }
    else {
        $glzFails++
        if ($glzFails -ge 3) {
            Get-Process glazewm -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
            Start-Sleep -Milliseconds 600
            Start-Process "$scoop\glazewm\current\GlazeWM.exe"; $glzFails = 0
        }
    }
    if (-not (Alive 'AltSnap'))      { Start-Process "$scoop\altsnap\current\AltSnap.exe" }
    if (-not (Alive 'AutoHotkey64')) { Start-Process $ahk -ArgumentList "`"$cfg\wezterm-hotkey.ahk`"" }

    # PowerToys Command Palette host (the win+ctrl+space launcher). It sometimes
    # fails to come up at login (PowerToys starts but the CmdPal app doesn't),
    # which leaves the hotkey dead. Relaunch the app if its host process is gone;
    # it self-hides once another window takes focus, so a boot relaunch is invisible.
    if (-not (Alive 'Microsoft.CmdPal.UI')) {
        Start-Process 'shell:AppsFolder\Microsoft.CommandPalette_8wekyb3d8bbwe!App' -EA SilentlyContinue
    }

    # dwindle: fibonacci layout (child of GlazeWM, dies on GlazeWM restart)
    if (-not (AliveCmd '*glazewm-dwindle*')) {
        Start-Process pwsh -ArgumentList '-NoProfile','-WindowStyle','Hidden','-File',"$cfg\glazewm-dwindle.ps1" -WindowStyle Hidden
    }
    # ShadowPlay WGC recorder (only launch if none -> never duplicates)
    if (-not (Alive 'shadowplay-wgc')) {
        Start-Process wscript.exe -ArgumentList "`"$cfg\shadowplay-wgc.vbs`""
    }
    # glaze-bar: one per monitor. Each bar is single-instance (named mutex per --x),
    # so launching both whenever fewer than two run respawns only the missing one.
    if ((AliveCount 'glaze-bar') -lt 2 -and (Test-Path $bar)) {
        Start-Process $bar -ArgumentList '--x','0','--width','1920'
        Start-Process $bar -ArgumentList '--x','1920','--width','2560'
    }
    # ws-slide owns Super+1..9 for the workspace-slide animation. If it dies, those
    # hotkeys go dead (GlazeWM no longer binds them), so keep it up. Single-instance
    # via a named mutex, so a duplicate launch is a harmless no-op.
    if (-not (Alive 'ws-slide') -and (Test-Path $wsslide)) { Start-Process $wsslide }

    try { [W.K]::EmptyWorkingSet([W.K]::GetCurrentProcess()) | Out-Null } catch {}
    Start-Sleep -Seconds 30
}
