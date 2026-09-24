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

# ---- Ojo: que modelo toca segun si hay un juego delante --------------------
#
# EL PROBLEMA, medido el 2026-09-22: el 8B con vision ocupa 11.418 de los
# 12.282 MiB de la tarjeta. Cuando ademas arranca un juego, la VRAM se acaba y
# el driver de Windows -- desde la version 536.40 -- NO da error: derrama a RAM
# del sistema por PCIe en silencio. Aquella sesion paso de 9 s a 222 s por
# frase, con la GPU al 99% y 343 W, sin un solo mensaje que lo explicara.
#
# LA SALIDA no es aparcar a Ojo mientras juegas, porque quieres preguntarle
# DURANTE la partida. Es que durante la partida no necesita ver: League sirve
# sus propios datos en https://127.0.0.1:2999 (ver `lol.ps1`) con los campeones,
# los items y el oro exactos. Sin vision no hace falta mmproj ni modelo de
# vision, y el 4B de texto cabe de sobra.
#
# Medido el 2026-09-22 (tras la auditoria: 8B en Q6_K, KV q8_0, sin
# calentamiento):
#   8B con vision    8.279 MiB,  62 tok/s   <- ~2.700 libres con el escritorio
#   4B de texto      3.619 MiB,  94 tok/s   <- ~7.600 libres
# El 8B aguanta ya que otra aplicacion le quite ~2,3 GB sin frenarse; LoL pide
# ~4,5, asi que en partida sigue haciendo falta el de texto.
#
# POR QUE AQUI Y NO CON UNA SUSCRIPCION A EVENTOS DE PROCESO: el supervisor ya
# late cada 30 s y ya vigila esta fila. `LeagueClient.exe` vive MINUTOS antes de
# que empiece la partida, asi que medio minuto de margen sobra de largo. Si
# midiendo resulta que no sobra, entonces si toca un `Win32_ProcessStartTrace`,
# y sustituye a esto en vez de sumarse.
#
# SIN Battle.net: es un lanzador que vive en la bandeja. Con el en la lista,
# Ojo se quedo sin vision una noche entera sin ningun juego abierto.
# SIN Hearthstone (2026-09-23): medido con el juego abierto, 406 MiB de VRAM, y
# el 8B con la voz aguanta ~2,2 GB de otra aplicacion. Ahi Ojo conserva la
# vision, que es lo util: no hay API de datos y ver las cartas si importa.
$OJO_JUEGOS = @('LeagueClient', 'League of Legends')

function Get-OjoPerfil {
    foreach ($j in $OJO_JUEGOS) {
        if (Get-Process -Name $j -EA SilentlyContinue) { return '4b-texto' }
    }
    '8b'
}

# La build del campeon de Ojo, precargada cuando ARRANCA el juego (un proceso
# "League of Legends" que no habiamos visto). En ARAM Mayhem la primera
# eleccion de aumento sale nada mas empezar, y bajar la clasificacion de op.gg
# tarda ~10 s: durante la pantalla de carga sobra. Una vez por proceso.
$script:OjoPrecargado = 0
function Invoke-OjoPrecarga {
    $j = Get-Process -Name 'League of Legends' -EA SilentlyContinue | Select-Object -First 1
    if (-not $j -or $j.Id -eq $script:OjoPrecargado) { return }
    $script:OjoPrecargado = $j.Id
    Write-RiceLog "juego arrancado (pid $($j.Id)); se precarga la build" 'ojo-vlm'
    Start-Process pwsh -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'D:\2026-projects\ojo\builds.ps1', '-Precargar' -WindowStyle Hidden
}

# El proceso del modelo de Ojo: el que escucha en el 8099. Uno o ninguno.
function Get-OjoServidor {
    @(Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" -EA SilentlyContinue |
      Where-Object { $_.CommandLine -match '--port\s+8099\b' })
}

# Si el perfil puesto no es el que toca, lo cambia EN ESTE LATIDO y devuelve
# $true (ha actuado). Si ya es el correcto, $false.
#
# Se mira la LINEA DE COMANDOS y no una variable nuestra: el servidor puede
# haberlo arrancado cualquiera -- ojo.ps1 a mano, una sesion anterior -- y lo
# unico que sabe la verdad es el proceso que hay puesto.
#
# POR QUE CAMBIA AQUI MISMO y no declarandose enfermo: medido con una partida
# falsa, la version anterior tardaba 57 s en pasar al modelo de texto y 154 s
# en volver a la vision. Hacian falta dos latidos fallidos (Fails = 2), y tras
# arrancar el de texto, 120 s de Grace durante los que el supervisor ni miraba.
function Update-OjoPerfil {
    $p = Get-OjoServidor
    if ($p.Count -ne 1) { return $false }
    $esTexto = $p[0].CommandLine -match 'Qwen3\.5-4B'
    $quiere = Get-OjoPerfil
    if ($esTexto -eq ($quiere -eq '4b-texto')) { return $false }
    Write-RiceLog ("perfil equivocado (puesto={0}, toca={1}); se cambia ya" -f $(if ($esTexto) { '4b-texto' } else { '8b' }), $quiere) 'ojo-vlm'
    Stop-Process -Id $p[0].ProcessId -Force -EA SilentlyContinue
    # Esperar a que muera de verdad: ojo.ps1 se niega a lanzar otro si aun ve
    # uno en el 8099 (asi se evitan dos modelos a la vez).
    Wait-Process -Id $p[0].ProcessId -Timeout 10 -EA SilentlyContinue
    $null = Start-RiceComponent ($Components | Where-Object Name -eq 'ojo-vlm')
    $true
}

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

    # Estimador de gasto electrico. Muestrea la potencia y la integra, asi que
    # lo que mide es el tiempo que pasa VIVO: cada minuto que este caido es un
    # minuto que no aparece en el total del dia. De ahi que lo vigile el
    # supervisor en vez de dejarlo en la carpeta de Inicio -- un cuelgue
    # silencioso no se notaria hasta mirar el recibo.
    #
    # Es una app de consola y no de ventana: el supervisor la lanza con
    # -WindowStyle Hidden, y asi `consumo --hoy` sigue imprimiendo cuando se
    # llama a mano desde una terminal.
    @{ Name = 'consumo'; Check = 'Process'; Match = 'consumo'
       Path = { Get-RiceExe 'consumo.exe' } }

    # ---- Ojo (D:\2026-projects\ojo) -------------------------------------
    #
    # (las funciones Get-OjoPerfil / Update-OjoPerfil viven arriba del todo,
    #  junto al resto de ayudantes)
    #
    # Las tres piezas van por MUTEX o por sonda, nunca por nombre de proceso a
    # secas, y cada una tiene su motivo:
    #
    #   el oido    es un python.exe, y el backend de voicebox tambien
    #   el atajo   es un AutoHotkey64, y wezterm-hotkey tambien
    #   el modelo  es un llama-server, y rice-llm levanta otro para el 35B
    #
    # Comprobar por nombre daria por vivo lo que esta muerto en los tres casos.

    @{ Name = 'ojo-oido'; Check = 'Mutex'; Match = 'Global\ojo-oido'
       Path = { 'D:\2026-projects\ojo\stt\.venv\Scripts\python.exe' }
       Args = { @('D:\2026-projects\ojo\stt\escuchar.py') }
       Grace = 30
       Health = { try { [bool](Invoke-RestMethod 'http://127.0.0.1:17494/salud' -TimeoutSec 2).ok } catch { $false } }
       # Kill PROPIO y no opcional: con Check='Mutex', el Match es el nombre de
       # un mutex, y el Get-Process por defecto no mataria nada. La instancia
       # colgada seguiria viva y la nueva se suicidaria al no poder tomarlo.
       Kill = { Get-CimInstance Win32_Process -Filter "Name='python.exe'" -EA SilentlyContinue |
                Where-Object { $_.CommandLine -match 'escuchar\.py' } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue } } }

    # La voz: Pocket TTS "lola" en CPU (gano la escucha a ciegas 3, 2026-09-24;
    # 0 de VRAM), residente. Reserva: la F2 en GPU con el venv de voz\ y
    # '--motor f2'. Mutex y Kill propio por lo mismo que el oido: es otro python.exe.
    @{ Name = 'ojo-voz'; Check = 'Mutex'; Match = 'Global\ojo-voz'
       Path = { 'F:\ai\tts\pocket\.venv\Scripts\python.exe' }
       Args = { @('D:\2026-projects\ojo\voz\servidor_voz.py', '--motor', 'pocket') }
       Grace = 30
       Health = { try { [bool](Invoke-RestMethod 'http://127.0.0.1:8098/salud' -TimeoutSec 2).ok } catch { $false } }
       Kill = { Get-CimInstance Win32_Process -Filter "Name='python.exe'" -EA SilentlyContinue |
                Where-Object { $_.CommandLine -match 'servidor_voz\.py' } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue } } }

    # El buscador: SearXNG local (solo DuckDuckGo) para que Ojo busque y cite en
    # vez de inventar. Mutex desde ojo\buscador.py; Kill propio (es un python).
    @{ Name = 'ojo-buscador'; Check = 'Mutex'; Match = 'Global\ojo-buscador'
       Path = { 'F:\ai\searxng\python\pythonw.exe' }
       Args = { @('D:\2026-projects\ojo\buscador.py') }
       Grace = 30
       Health = { try { $null = Invoke-WebRequest 'http://127.0.0.1:8888/healthz' -UseBasicParsing -TimeoutSec 3; $true } catch { $false } }
       Kill = { Get-CimInstance Win32_Process -Filter "Name='pythonw.exe' OR Name='python.exe'" -EA SilentlyContinue |
                Where-Object { $_.CommandLine -match 'buscador\.py|searx\.webapp' } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue } } }

    # Sin Health a proposito: un AHK cargado o esta vivo o no esta, y no hay
    # sonda barata que distinga mas. Al no haber Health nunca se llega al Kill,
    # asi que tampoco le hace falta uno propio.
    @{ Name = 'ojo-hotkey'; Check = 'Mutex'; Match = 'Global\ojo-hotkey'
       Path = { "$($Rice.ScoopApps)\autohotkey\current\v2\AutoHotkey64.exe" }
       Args = { @('D:\2026-projects\ojo\ojo-hotkey.ahk') } }

    # El Match es ambiguo A PROPOSITO -- hay dos llama-server posibles -- y
    # quien decide de verdad es la sonda contra el puerto 8099. El Kill acota a
    # quien toca: sin el, el Get-Process por defecto se llevaria por delante el
    # 35B del Win+Space.
    #
    # Grace 0 y la espera por carga lenta DENTRO de Health, mirando la edad del
    # proceso. Antes era Grace 120, y Grace no deja llamar a Health en absoluto:
    # durante esos dos minutos tampoco se comprobaba el perfil, y volver a la
    # vision tras una partida tardaba 154 s. La carga son 4-5 s desde NVMe; los
    # 120 s siguen ahi para el arranque de sesion, cuando compite por el disco.
    @{ Name = 'ojo-vlm'; Check = 'Process'; Match = 'llama-server'
       Path = { (Get-Command pwsh -EA SilentlyContinue).Source ?? "$env:ProgramFiles\PowerShell\7\pwsh.exe" }
       Args = { @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden',
                  '-File', 'D:\2026-projects\ojo\ojo.ps1', '-Servidor',
                  '-Modelo', (Get-OjoPerfil)) }
       Grace = 0; Fails = 2
       # La primera clausula mira el 8080 para NO declarar enfermo al de ojo
       # cuando el Win+Space le ha quitado la tarjeta a proposito (no caben los
       # dos: 11,4 + 9,3 sobre 12,28).
       #
       # LIMITE MEDIDO, y conviene saberlo: esto solo actua si el proceso ESTA
       # VIVO. `Step-RiceComponent` consulta `Health` unicamente en esa rama; si
       # el componente falta, lo arranca sin preguntar nada. Probado con un 8080
       # falso: mato el de ojo y el supervisor lo relanzo igual.
       #
       # No es peligroso porque `ojo.ps1` tiene su propia guarda y se niega a
       # arrancar con el 8080 puesto -- eso si esta verificado. El coste es un
       # `pwsh` y una linea de log cada 30 s mientras dure. Para arreglarlo de
       # verdad haria falta que `Check` supiera mirar un puerto, y eso es tocar
       # `lib/rice-proc.ps1`, que es de todos.
       Health = {
           Invoke-OjoPrecarga
           try { $null = Invoke-RestMethod 'http://127.0.0.1:8080/v1/models' -TimeoutSec 2; return $true } catch { }
           # Primero el perfil: si toca otro, se cambia ya y no hay nada que sondear.
           if (Update-OjoPerfil) { return $true }
           $vivo = try { (Invoke-RestMethod 'http://127.0.0.1:8099/health' -TimeoutSec 3).status -eq 'ok' } catch { $false }
           if ($vivo) { return $true }
           # No contesta, pero si acaba de nacer esta cargando: no es un fallo.
           $p = Get-OjoServidor
           [bool]($p.Count -eq 1 -and ((Get-Date) - $p[0].CreationDate).TotalSeconds -lt 120)
       }
       Kill = { Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" -EA SilentlyContinue |
                Where-Object { $_.CommandLine -match '--port\s+8099\b' } |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -EA SilentlyContinue } } }
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
