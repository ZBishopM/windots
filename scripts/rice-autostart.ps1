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


# Launch everything at once. These used to go one at a time, each blocking on
# WaitWin until its window appeared -- up to 15s apiece plus a settle, so five
# apps could serialise ~85s of pure waiting at every login.
#
# What made that ordering necessary was placement: focus a workspace, start the
# app, wait for it to land. GlazeWM now does the placing itself (see the
# `move --workspace` window rules in its config), so order stops mattering and
# they can all start together. On this hardware they genuinely do run in
# parallel; the old script was not CPU-bound, it was waiting.
#
# `Proceso` = no lanzarlo si ya esta corriendo. Es la misma guarda que hubo que
# ponerle a taskbar.exe (ver el comentario mas abajo, y el commit c7f0ac7): en el
# inicio de sesion pueden coincidir dos vias -- esta y el arranque propio de la
# aplicacion -- y entonces salen dos ventanas.
#
# El caso de Firefox: el trim borro su entrada Run\Mozilla-Firefox-... pero dejo
# el visto bueno en StartupApproved marcado como HABILITADO, y Firefox reescribe
# esa entrada cuando quiere. Ya paso con OneDrive, que se actualizo y se volvio a
# poner solo en el arranque.
$parallel = @(
    @{ Path = 'shell:AppsFolder\Claude_pzs8sxrjxfjjc!Claude'; Proceso = 'Claude' }
    @{ Path = "$env:LOCALAPPDATA\Programs\Zed\Zed.exe";       Proceso = 'Zed' }
    @{ Path = 'C:\Program Files\Firefox Developer Edition\firefox.exe'; Proceso = 'firefox' }
)
foreach ($a in $parallel) {
    if ($a.Proceso) {
        $ya = @(Get-Process $a.Proceso -EA SilentlyContinue)
        if ($ya.Count) {
            # Se deja dicho: si esto sale en el log al iniciar sesion, hay una
            # segunda via de arranque y hay que buscarla, no taparla.
            Write-Host ("ya corria {0} ({1} procesos): no lo lanzo" -f $a.Proceso, $ya.Count)
            continue
        }
    }
    if ($a.Args) { Start-Process $a.Path -ArgumentList $a.Args -EA SilentlyContinue }
    else         { Start-Process $a.Path -EA SilentlyContinue }
}

# Esperar a que la ventana APAREZCA, no dos segundos por si acaso.
#
# Los dos son wezterm-gui, asi que una regla por nombre de proceso no distingue
# la del `claude` de la del `btop`, y al lanzarlas su titulo aun no esta puesto:
# por eso siguen en orden. Pero el orden solo exige esperar a que la primera
# exista, y eso se puede comprobar en vez de suponerlo. Eran dos `Start-Sleep
# -Seconds 2` fijos esperando a nada.
function WaitWezterm($antes, $sec = 6) {
    $fin = (Get-Date).AddSeconds($sec)
    while ((Get-Date) -lt $fin) {
        if ((Get-Process wezterm-gui -EA SilentlyContinue | Measure-Object).Count -gt $antes) { return }
        Start-Sleep -Milliseconds 120
    }
}

$n = (Get-Process wezterm-gui -EA SilentlyContinue | Measure-Object).Count
Focus 2
Start-Process $wez -ArgumentList 'start', '--', 'nu', '-e', 'claude-winrice'
WaitWezterm $n
# Segundo Claude, este en D:\2026-projects, retomando la conversacion de los
# proyectos en vez de empezar una nueva.
#
# El comando vive en config.nu (`claude-proyectos`) y no aqui por dos razones.
# Una: `nu -e` con un argumento que lleva ESPACIOS no ejecuta nada cuando lo
# lanza Start-Process -- el argumento llega intacto, se ve en la linea de
# comandos del proceso, y el -e se queda sin correr; sin espacios funciona
# siempre, que es la forma que `nu -e claude` lleva usando cada arranque.
# Dos: el termino de busqueda de la conversacion se cambia antes en la config
# de la shell que dentro de un array de PowerShell.
$n = (Get-Process wezterm-gui -EA SilentlyContinue | Measure-Object).Count
Focus 3
Start-Process $wez -ArgumentList 'start', '--cwd', 'D:\2026-projects', '--', 'nu', '-e', 'claude-proyectos'
WaitWezterm $n

$n = (Get-Process wezterm-gui -EA SilentlyContinue | Measure-Object).Count
Focus 5
Start-Process $wez -ArgumentList 'start', '--', 'pwsh', '-NoExit', '-Command', 'btop'
WaitWezterm $n

# -SettleMs 0: es el ultimo enfoque del script y detras no viene nada que pueda
# adelantarse, asi que los 400 ms por defecto se esperarian para nada.
Set-GlazeWorkspace -Index 1 -SettleMs 0 | Out-Null  # end on the primary workspace

# The Windows taskbar is hidden by taskbar.exe --watch, which the SUPERVISOR
# owns (see its component table). It used to be started here too, so at login
# both could fire and leave two watchers fighting over the same window.

# Una vez por semana: ha mergeado ya el PR #1392 de animaciones de GlazeWM?
#
# La puerta semanal se mira AQUI, no dentro del script. Lanzarlo siempre gastaba
# un pwsh entero -- 274 ms medidos -- para que el propio script comprobara la
# fecha y volviera sin hacer nada, seis de cada siete arranques. La comprobacion
# es un Test-Path y una resta; el proceso solo hace falta el septimo dia.
$animFlag  = "$env:USERPROFILE\.config\.glaze-anim-merged"
$animStamp = "$env:USERPROFILE\.config\.glaze-anim-checked"
$tocaMirar = -not (Test-Path $animFlag) -and (
    -not (Test-Path $animStamp) -or
    ((Get-Date) - (Get-Item $animStamp).LastWriteTime) -ge [TimeSpan]::FromDays(7)
)
if ($tocaMirar) {
    Start-Process pwsh -ArgumentList '-NoProfile', '-WindowStyle', 'Hidden', '-File', "$env:USERPROFILE\.config\glazewm-animcheck.ps1" -WindowStyle Hidden
}
