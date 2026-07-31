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

Write-RiceLog 'supervisor starting'

# NO se espera a GlazeWM aqui. Aqui habia un `Wait-GlazeIpcReady -TimeoutSec 90`
# antes de construir la tabla, y eso ataba TODOS los componentes al arranque del
# gestor de ventanas. De los once, solo la fila `glazewm` depende de el: notifyd,
# taskbar, launcher, ws-slide, shadowplay-wgc, altsnap y wezterm-hotkey no le
# piden nada. Con GlazeWM tardando o sin arrancar, el escritorio se quedaba sin
# supervisar entero -- y notifyd caido bajo No molestar significa que las
# notificaciones no aparecen en ningun sitio.
#
# La espera se movio a la fila `glazewm`, que ya tiene `Grace = 90` para lo mismo:
# su sonda de salud no cuenta como fallo hasta que pasa ese margen.
#
# El primer tick tampoco corre inmediatamente. La carpeta de Inicio esta lanzando
# GlazeWM, AltSnap, wezterm-hotkey y shadowplay-wgc en este mismo momento, y sin
# este margen el supervisor los veia ausentes y lanzaba una segunda copia: en el
# log del 29/07 se ve `[wezterm-hotkey] started` cuando el acceso directo ya lo
# habia arrancado, y `#SingleInstance Force` mataba al primero.
Start-Sleep -Seconds 8

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

    # The Win+Space search box. Resident on purpose -- a launcher that has to
    # start before it can search is one you stop using -- and it costs 0.00% CPU
    # idle, against the 267 MB and 59s of login that PowerToys' Command Palette
    # was charging for the same job.
    @{ Name = 'launcher'; Check = 'Process'; Match = 'launcher'
       Path = { Get-RiceExe 'launcher.exe' } }

    # dwindle: fibonacci layout (a child of GlazeWM, so it dies on a GlazeWM
    # restart). Checked by its own mutex rather than a WMI command-line match.
    # Ruta COMPLETA a pwsh, no 'pwsh' a secas: Start-RiceComponent valida con
    # Test-Path, que mira el sistema de archivos y no el PATH, asi que el nombre
    # suelto fallaba siempre con "missing binary" y dwindle se quedaba muerto
    # tras cada reinicio de GlazeWM (que mata a su hijo). Descubierto porque el
    # log lo repetia en cada tick.
    @{ Name = 'dwindle'; Check = 'Mutex'; Match = 'Global\glazewm-dwindle-ps'
       Path = { (Get-Command pwsh -ErrorAction SilentlyContinue).Source ?? "$env:ProgramFiles\PowerShell\7\pwsh.exe" }
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
#
# Y con comprobacion de salud, porque una barra puede quedarse VIVA Y PARADA. Le
# paso: el reloj se quedo congelado en 03:11 mientras el proceso seguia con
# Responding=True y su mutex tomado, o sea invisible para las dos formas de
# comprobar que habia aqui. La barra escribe bar-alive-<x>.txt desde su bucle de
# dibujo cada 5 s; si ese archivo envejece, es que dejo de pintar.
#
# Kill propio: el reinicio generico hace `Get-Process $Match`, y aqui Match es el
# nombre de un mutex, no de un proceso -- no mataria nada, y la instancia nueva
# se suicidaria contra el mutex de la vieja. Ademas hay que matar SOLO la barra
# de este monitor, que se distingue por su --x en la linea de comandos.
foreach ($m in $Rice.Monitors) {
    $mon = $m
    $Components += @{
        Name        = "glaze-bar@$($mon.X)"
        Check       = 'Mutex'; Match = "Global\glaze-bar-$($mon.X)"
        Path        = { Get-RiceExe 'glaze-bar.exe' }
        Args        = { @('--x', $mon.X, '--width', $mon.Width) }.GetNewClosure()
        MaxRestarts = 10
        Health      = {
            $f = Join-Path $Rice.Config "bar-alive-$($mon.X).txt"
            if (-not (Test-Path $f)) { return $true }   # version vieja sin latido
            ((Get-Date) - (Get-Item $f).LastWriteTime).TotalSeconds -lt 30
        }.GetNewClosure()
        Kill        = {
            Get-CimInstance Win32_Process -Filter "Name='glaze-bar.exe'" -EA SilentlyContinue |
                Where-Object { $_.CommandLine -match "--x\s+$($mon.X)\b" } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue }
        }.GetNewClosure()
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
