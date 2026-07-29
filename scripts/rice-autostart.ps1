# Autostart the working layout ONCE at login. No persistent window_rules -- each
# app is placed only now, by focusing its workspace and waiting for its window to
# appear before moving on (so it lands there). Open apps later go wherever you are.

$wez = 'C:\Program Files\WezTerm\wezterm-gui.exe'

# The IPC client lives in lib/rice-ipc.ps1 now. This script's own copy was the
# last one still using 'localhost', paired with the shortest connect budget in
# the tree -- so it usually lost the race to the ~2.1s IPv6 timeout, the catch{}
# swallowed it, and the next app opened on whatever workspace happened to be
# focused. That was the "my apps came up on the wrong workspace" symptom.
. "$env:USERPROFILE\.config\lib\rice-paths.ps1"
. "$env:USERPROFILE\.config\lib\rice-ipc.ps1"

# Esperar a que GlazeWM CONTESTE, no doce segundos por si acaso.
#
# Aqui habia un `Start-Sleep -Seconds 12` con el comentario "let GlazeWM +
# dwindle + the bars settle". Era una adivinanza, y de las caras: en el ultimo
# inicio de sesion GlazeWM ya estaba arriba a las 09:32:11 y este script no
# lanzo nada hasta las 09:32:27. Diez segundos de reloj mirando al techo.
#
# `Wait-GlazeIpcReady` se escribio precisamente para sustituir estas esperas
# fijas -- lo dice su propio comentario en lib/rice-ipc.ps1 -- pero la
# migracion nunca llego a este archivo.
#
# Lo unico que de verdad tiene que estar listo antes es GlazeWM: es quien coloca
# cada ventana por window_rules segun aparece, y quien responde a los `Focus` de
# mas abajo. Las barras y dwindle reaccionan a eventos y no bloquean a nadie.
#
# Si no contesta se sigue igualmente: mejor las aplicaciones mal colocadas que
# ninguna aplicacion.
if (-not (Wait-GlazeIpcReady -TimeoutSec 60)) {
    Write-Host 'GlazeWM no respondio; sigo de todas formas'
}

function Focus($n) {
    if (-not (Set-GlazeWorkspace -Index $n)) { Write-Host "Focus ${n}: no IPC" }
}

# Launch then block until the app's window exists (login is a clean slate, so the
# process isn't already running), so focus stays on the target ws until it lands.
function WaitWin($proc, $sec = 15) {
    $end = (Get-Date).AddSeconds($sec)
    while (-not (Get-Process $proc -EA SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 }) -and (Get-Date) -lt $end) {
        Start-Sleep -Milliseconds 400
    }
    Start-Sleep -Milliseconds 700  # settle in the workspace
}

# Launch everything at once. These used to go one at a time, each blocking on
# WaitWin until its window appeared -- up to 15s apiece plus a settle, so five
# apps could serialise ~85s of pure waiting at every login.
#
# What made that ordering necessary was placement: focus a workspace, start the
# app, wait for it to land. GlazeWM now does the placing itself (see the
# `move --workspace` window rules in its config), so order stops mattering and
# they can all start together. On this hardware they genuinely do run in
# parallel; the old script was not CPU-bound, it was waiting.
$parallel = @(
    @{ Path = 'shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude' }
    @{ Path = "$env:LOCALAPPDATA\Programs\Zed\Zed.exe" }
    @{ Path = 'C:\Program Files\Firefox Developer Edition\firefox.exe' }
)
foreach ($a in $parallel) {
    if ($a.Args) { Start-Process $a.Path -ArgumentList $a.Args -EA SilentlyContinue }
    else         { Start-Process $a.Path -EA SilentlyContinue }
}

# The terminals stay ordered. Both are wezterm-gui, so a process-name rule cannot
# tell the `claude` window from the `btop` one, and at launch their titles are not
# set yet either. They come up fast, so the cost of keeping this sequential is
# small -- unlike the block above.
Focus 2
Start-Process $wez -ArgumentList 'start', '--', 'pwsh', '-NoExit', '-Command', 'claude'
Start-Sleep -Seconds 2
Focus 5
Start-Process $wez -ArgumentList 'start', '--', 'pwsh', '-NoExit', '-Command', 'btop'
Start-Sleep -Seconds 2

Focus 1  # end on the primary workspace

# The Windows taskbar is hidden by taskbar.exe --watch, which the SUPERVISOR
# owns (see its component table). It used to be started here too, so at login
# both could fire and leave two watchers fighting over the same window.

# Once at login: has GlazeWM's animation PR #1392 merged yet? Toast if so (fail-silent).
Start-Process pwsh -ArgumentList '-NoProfile', '-WindowStyle', 'Hidden', '-File', "$env:USERPROFILE\.config\glazewm-animcheck.ps1" -WindowStyle Hidden
