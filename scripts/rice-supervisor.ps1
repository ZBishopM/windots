# Keeps the rice's always-on processes alive. Every 30s it checks each component
# and relaunches any that died (crash, manual kill, GlazeWM restart killing its
# child dwindle, etc.) so a dead piece self-heals within a minute instead of
# staying dead until the next login.
#
# Components are a data table, not hand-written control flow: adding one is a
# single row. The previous version inlined six components with four different
# liveness idioms and five Start-Process shapes in a 44-line loop body, guarded
# nothing but two of them with Test-Path, and logged nothing at all -- so when it
# died, it died silently.

$ErrorActionPreference = 'Continue'
. "$env:USERPROFILE\.config\lib\rice-paths.ps1"
. "$env:USERPROFILE\.config\lib\rice-ipc.ps1"
. "$env:USERPROFILE\.config\lib\rice-proc.ps1"

# Single instance. The catch matters: if a previous supervisor was killed while
# holding this mutex, WaitOne throws AbandonedMutexException, and unhandled that
# terminated the script instantly -- leaving the whole rice unsupervised until
# the next login. The mutex IS acquired; the exception only reports the abandon.
$mutex = New-Object System.Threading.Mutex($false, 'Global\rice-supervisor')
try { if (-not $mutex.WaitOne(0)) { exit } }
catch [System.Threading.AbandonedMutexException] { }

Add-Type -Namespace W -Name K -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("psapi.dll")] public static extern bool EmptyWorkingSet(System.IntPtr h);
[System.Runtime.InteropServices.DllImport("kernel32.dll")] public static extern System.IntPtr GetCurrentProcess();
'@ -EA SilentlyContinue

# Wait for GlazeWM to answer rather than sleeping a fixed guess, so the first
# tick can't land in the middle of the login layout and judge a warming-up WM.
Write-RiceLog 'supervisor starting'
[void](Wait-GlazeIpcReady -TimeoutSec 90)

$Components = @(
    # GlazeWM can wedge: the process stays alive but its IPC (and keybinds) stop
    # responding, which a plain process check misses -- hence the Health probe.
    @{ Name    = 'glazewm'
       Check   = 'Process'; Match = 'GlazeWM'
       Health  = { Test-GlazeIpcAlive }
       Grace   = 90     # don't judge it while it is still coming up
       Fails   = 3      # ~90s of consecutive failures before intervening
       Path    = { "$($Rice.ScoopApps)\glazewm\current\GlazeWM.exe" } }

    @{ Name = 'altsnap'; Check = 'Process'; Match = 'AltSnap'
       Path = { "$($Rice.ScoopApps)\altsnap\current\AltSnap.exe" } }

    @{ Name = 'wezterm-hotkey'; Check = 'Process'; Match = 'AutoHotkey64'
       Path = { "$($Rice.ScoopApps)\autohotkey\current\v2\AutoHotkey64.exe" }
       Args = { @("$($Rice.Config)\wezterm-hotkey.ahk") } }

    # The Win+Space search box. Resident on purpose: a launcher that has to start
    # before it can search is one you stop using -- but resident here is a few MB
    # trimmed, not the 267 MB PowerToys' Command Palette held, and it starts in
    # about a second instead of the 59s CmdPal spent at every login.
    @{ Name = 'launcher'; Check = 'Process'; Match = 'launcher'
       Path = { Get-RiceExe 'launcher.exe' } }

    # dwindle: fibonacci layout (a child of GlazeWM, so it dies on a GlazeWM
    # restart). Checked by its own mutex rather than a WMI command-line match.
    @{ Name = 'dwindle'; Check = 'Mutex'; Match = 'Global\glazewm-dwindle-ps'
       Path = { 'pwsh' }
       Args = { @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden',
                  '-File', "$($Rice.Config)\glazewm-dwindle.ps1") } }

    # WGC rolling recorder (single-instance via its own mutex).
    @{ Name = 'shadowplay-wgc'; Check = 'Process'; Match = 'shadowplay-wgc'
       Path = { Get-RiceExe 'shadowplay-wgc.exe' } }

    # ws-slide owns Super+1..9 for the workspace-slide animation, and GlazeWM no
    # longer binds those keys -- if this dies, workspace switching dies with it.
    @{ Name = 'ws-slide'; Check = 'Process'; Match = 'ws-slide'
       Path = { Get-RiceExe 'ws-slide.exe' } }

    # Keeps the Windows taskbar hidden. Explorer re-shows it on every hover, so
    # this has to stay resident; if it dies the taskbar creeps back on its own.
    # It honours the marker file, so Win+Shift+B still wins over it.
    @{ Name = 'taskbar'; Check = 'Process'; Match = 'taskbar'
       Path = { Get-RiceExe 'taskbar.exe' }; Args = { @('--watch') } }

    # Redraws every Windows notification with the rice's toast. Supervised, not
    # a plain Startup shortcut, because the failure mode is silent and total:
    # Do Not Disturb is what suppresses the stock blue banners, and DND does not
    # care whether notifyd is alive -- if this dies, notifications stop appearing
    # ANYWHERE except the Notification Center (Win+N) until it is back. 30s.
    #
    # No MaxRestarts on purpose. A cap would eventually give up and leave the
    # machine permanently silent; notifyd already parks instead of exiting when
    # its permissions are missing, so it does not respawn-loop.
    @{ Name = 'notifyd'; Check = 'Process'; Match = 'notifyd'
       Path = { Get-RiceExe 'notifyd.exe' } }
)

# One bar per monitor, each with its own single-instance mutex keyed by --x.
# The old check was `count -lt 2 -> launch both`, which on a single-monitor
# machine (or with the second display asleep) never reached 2 and therefore
# spawned two processes every 30s forever.
foreach ($m in $Rice.Monitors) {
    $mon = $m
    $Components += @{
        Name        = "glaze-bar@$($mon.X)"
        Check       = 'Mutex'; Match = "Global\glaze-bar-$($mon.X)"
        Path        = { Get-RiceExe 'glaze-bar.exe' }
        Args        = { @('--x', $mon.X, '--width', $mon.Width) }.GetNewClosure()
        MaxRestarts = 10
    }
}

$state = @{}
$tick = 0
while ($true) {
    $snap = Get-RiceProcessSnapshot     # one enumeration for the whole tick
    foreach ($c in $Components) {
        try { Step-RiceComponent $c $snap $state }
        catch { Write-RiceLog "step failed: $_" $c.Name }
    }
    # Trim on a wallclock cadence, not every tick: every trimmed page has to soft
    # fault back in afterwards.
    if (($tick % 20) -eq 0) {
        try { [W.K]::EmptyWorkingSet([W.K]::GetCurrentProcess()) | Out-Null } catch { }
    }
    $tick++
    Start-Sleep -Seconds 30
}
