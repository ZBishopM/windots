# Apaga procesos de fondo que estan corriendo sin hacer falta. Necesita
# administrador: todo lo de aqui vive en HKLM, en servicios o en ProgramData.
#
# Nada se desinstala. Todo lo que hace es reversible con -Undo, y cada bloque
# dice cual es la contrapartida real de apagarlo.
#
#   rice-trim-background.ps1          apaga
#   rice-trim-background.ps1 -Undo    lo deja como estaba
[CmdletBinding()]
param([switch]$Undo)

$ErrorActionPreference = 'Continue'
$id  = [Security.Principal.WindowsIdentity]::GetCurrent()
if (-not (New-Object Security.Principal.WindowsPrincipal $id).IsInRole(
        [Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Hace falta PowerShell como administrador.' -ForegroundColor Yellow
    exit 1
}

# --- 1. Parsec ------------------------------------------------------------
# El servicio existe para ALOJAR sesiones, es decir para que alguien se conecte
# a esta maquina. Comprobado que no escucha en ningun puerto, asi que solo esta
# ocupando memoria. Conectarse HACIA fuera sigue funcionando: al abrir Parsec el
# servicio arranca solo, que es lo que significa Manual.
Write-Host '== Parsec =='
if ($Undo) {
    Set-Service Parsec -StartupType Automatic
    Start-Service Parsec
    Write-Host '   servicio Parsec: Automatico, arrancado'
} else {
    Stop-Service Parsec -Force -EA SilentlyContinue
    Set-Service Parsec -StartupType Manual
    Get-Process parsecd -EA SilentlyContinue | Stop-Process -Force
    Write-Host '   servicio Parsec: Manual, parado'
}

# --- 1b. G HUB -----------------------------------------------------------
# Quitarle el arranque no bastó. Medido después: LGHUBUpdaterService estaba en
# Running pese a tener arranque Manual, y es quien vuelve a levantar
# lghub_system_tray.exe, que a su vez levanta lghub_agent.exe. El interruptor de
# "Aplicaciones de inicio" no lo ve porque no pasa por ahí.
#
# El G502 guarda DPI, botones e iluminación en el propio ratón, así que apagar
# esto no cambia cómo se comporta. Sólo hay que volver a abrir G HUB a mano para
# CAMBIAR un perfil, y entonces el servicio arranca solo.
Write-Host '== G HUB =='
if ($Undo) {
    Set-Service LGHUBUpdaterService -StartupType Manual
    Write-Host '   LGHUBUpdaterService: Manual'
} else {
    Stop-Service LGHUBUpdaterService -Force -EA SilentlyContinue
    Set-Service LGHUBUpdaterService -StartupType Disabled
    Get-Process lghub, lghub_agent, lghub_system_tray, lghub_updater -EA SilentlyContinue | Stop-Process -Force
    Write-Host '   LGHUBUpdaterService: Deshabilitado, y procesos cerrados'
}

# --- 2. Icono de Seguridad de Windows ------------------------------------
# Solo el ICONO de la bandeja. Defender, su servicio y sus analisis siguen
# exactamente igual: esto no toca la proteccion, toca el icono.
Write-Host '== icono de Defender =='
$run = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run'
$bak = 'HKLM:\SOFTWARE\rice-trim'
if ($Undo) {
    $v = (Get-ItemProperty $bak -Name SecurityHealth -EA SilentlyContinue).SecurityHealth
    if ($v) { Set-ItemProperty $run -Name SecurityHealth -Value $v; Write-Host '   restaurado' }
} else {
    $v = (Get-ItemProperty $run -Name SecurityHealth -EA SilentlyContinue).SecurityHealth
    if ($v) {
        if (-not (Test-Path $bak)) { New-Item $bak -Force | Out-Null }
        Set-ItemProperty $bak -Name SecurityHealth -Value $v
        Remove-ItemProperty $run -Name SecurityHealth
        Get-Process SecurityHealthSystray -EA SilentlyContinue | Stop-Process -Force
        Write-Host '   quitado del arranque (valor guardado en HKLM\rice-trim)'
    } else { Write-Host '   ya no estaba' }
}

# --- 3. Tareas programadas de Office -------------------------------------
# Once tareas: actualizaciones automaticas, telemetria de sostenibilidad,
# mantenimiento y "background push". De ahi sale el aviso de "Office se esta
# instalando en segundo plano". Contrapartida real: Office deja de actualizarse
# solo, y hay que actualizarlo a mano desde Archivo > Cuenta.
Write-Host '== tareas de Office =='
$tasks = Get-ScheduledTask | Where-Object {
    $_.TaskPath -like '*Office*' -or $_.TaskName -match '^Office |SustainabilityTelemetry|RecoverabilityToastTask'
}
foreach ($t in $tasks) {
    try {
        if ($Undo) { Enable-ScheduledTask -TaskPath $t.TaskPath -TaskName $t.TaskName | Out-Null }
        else       { Disable-ScheduledTask -TaskPath $t.TaskPath -TaskName $t.TaskName | Out-Null }
        Write-Host ("   {0} {1}" -f $(if ($Undo) {'+'} else {'-'}), $t.TaskName)
    } catch { Write-Host ("   ! {0}: {1}" -f $t.TaskName, $_.Exception.Message) }
}

# --- 4. Arranques muertos de ProgramData ---------------------------------
# Estan en la carpeta de Inicio para todos los usuarios y ni siquiera llegan a
# quedarse corriendo. No se borran: se apartan, asi que devolverlos es
# arrastrarlos de vuelta.
#
# El almacen esta FUERA de la carpeta de Inicio, y eso importa. La primera
# version usaba un subdirectorio dentro de ella, con el razonamiento de que
# "Windows no recorre subcarpetas". Es verdad a medias y por eso costo un bug:
# no ejecuta los accesos directos de dentro, pero SI ABRE LA CARPETA EN EL
# EXPLORADOR al iniciar sesion. Cada arranque saltaba una ventana de
# 'desactivado-por-rice'. Cualquier cosa dentro de Inicio se "ejecuta", y para
# una carpeta ejecutar significa abrirla.
#
# Cloudflare WARP NO esta en la lista a proposito: lo pediste explicitamente.
Write-Host '== arranques muertos (todos los usuarios) =='
$sf   = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Startup"
$off  = "$env:ProgramData\rice\arranques-desactivados"
$dead = @('CodeMeter Control Center.lnk', 'ScpToolkit Tray Notifications.lnk')
if (-not (Test-Path $off)) { New-Item $off -ItemType Directory -Force | Out-Null }

# Migracion del sitio viejo. Se hace siempre, tambien con -Undo, porque lo que
# hay que quitar de Inicio es la CARPETA en si.
$legacy = Join-Path $sf 'desactivado-por-rice'
if (Test-Path $legacy) {
    Get-ChildItem $legacy -Force -EA SilentlyContinue |
        ForEach-Object { Move-Item $_.FullName (Join-Path $off $_.Name) -Force -EA SilentlyContinue }
    Remove-Item $legacy -Recurse -Force -EA SilentlyContinue
    Write-Host "   movido fuera de Inicio -> $off"
}

foreach ($n in $dead) {
    if ($Undo) {
        $src = Join-Path $off $n
        if (Test-Path $src) { Move-Item $src (Join-Path $sf $n) -Force; Write-Host "   + $n" }
    } else {
        $src = Join-Path $sf $n
        if (Test-Path $src) { Move-Item $src (Join-Path $off $n) -Force; Write-Host "   - $n" }
    }
}

# --- 5. Overlay de NVIDIA -------------------------------------------------
# ~696 MB medidos: 5 procesos de 'NVIDIA Overlay' (476 MB) mas los 4 de
# 'nvcontainer' (220 MB) que los alojan. Es el Alt+Z, el contador de FPS y el
# ShadowPlay de NVIDIA -- justo lo que este escritorio ya reemplazo con
# shadowplay-wgc, que ocupa 177 MB y guarda con Alt+F10.
#
# NVDisplay.ContainerLocalSystem NO se toca, y la diferencia importa: ese es el
# contenedor del DRIVER DE PANTALLA. De ahi salen el panel de control de NVIDIA,
# G-Sync y la gestion de resoluciones. Apagarlo no ahorra overlay, rompe la
# pantalla. Se parecen en el nombre y no en lo que hacen.
Write-Host '== overlay de NVIDIA =='
if ($Undo) {
    Set-Service NvContainerLocalSystem -StartupType Automatic -EA SilentlyContinue
    Start-Service NvContainerLocalSystem -EA SilentlyContinue
    Write-Host '   NvContainerLocalSystem: Automatico, arrancado'
} else {
    Stop-Service NvContainerLocalSystem -Force -EA SilentlyContinue
    Set-Service NvContainerLocalSystem -StartupType Disabled -EA SilentlyContinue
    Get-Process 'NVIDIA Overlay','nvcontainer','nvsphelper64' -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    Write-Host '   NvContainerLocalSystem: Deshabilitado, parado (overlay incluido)'
}

# El autoactualizador de la NVIDIA App, misma categoria que los de SCP y Meta
# Horizon que ya se apagaron por avisar constantemente.
$t = Get-ScheduledTask -EA SilentlyContinue | Where-Object { $_.TaskName -like 'NVIDIA App SelfUpdate*' }
foreach ($x in $t) {
    if ($Undo) { Enable-ScheduledTask -TaskPath $x.TaskPath -TaskName $x.TaskName -EA SilentlyContinue | Out-Null }
    else       { Disable-ScheduledTask -TaskPath $x.TaskPath -TaskName $x.TaskName -EA SilentlyContinue | Out-Null }
    Write-Host ("   {0} {1}" -f $(if ($Undo) {'+'} else {'-'}), $x.TaskName)
}

# --- 6. Indice de Windows -------------------------------------------------
# El launcher tiene su propio indice de archivos (crates/launcher/src/files.rs),
# que cubre el 100% de los discos. El catalogo de WSearch tenia ~196.000 de las
# ~2,17M entradas de esta maquina -- un 12% -- porque su ambito excluye AppData,
# todos los dotfolders y D: entero. Ademas solo indexa por prefijo, asi que ni
# siquiera puede servir busqueda difusa.
#
# LA CONTRAPARTIDA ES REAL: la busqueda del menu Inicio y la del Explorador
# dependen de este servicio y dejan de encontrar archivos. Win+Space no.
Write-Host '== indice de Windows =='
if ($Undo) {
    Set-Service WSearch -StartupType Automatic -EA SilentlyContinue
    Start-Service WSearch -EA SilentlyContinue
    Write-Host '   WSearch: Automatico, arrancado'
} else {
    Stop-Service WSearch -Force -EA SilentlyContinue
    Set-Service WSearch -StartupType Disabled -EA SilentlyContinue
    Write-Host '   WSearch: Deshabilitado, parado'
}

# --- 7. Tareas de inicio de sesion muertas --------------------------------
# MSIAfterburner: la tarea existe y el ejecutable NO. Se dispara en cada inicio
# de sesion y devuelve 0x80070002 (fichero no encontrado). Lleva fallando desde
# que se desinstalo el programa.
#
# OneDrive Startup Task: la segunda via de arranque de OneDrive. El trim de las
# claves Run solo miraba HKCU\...\Run, asi que esta tarea nunca se toco.
Write-Host '== tareas de inicio muertas =='
foreach ($n in 'MSIAfterburner', 'OneDrive Startup Task') {
    $tasks = Get-ScheduledTask -EA SilentlyContinue | Where-Object { $_.TaskName -like "$n*" }
    foreach ($x in $tasks) {
        if ($Undo) { Enable-ScheduledTask -TaskPath $x.TaskPath -TaskName $x.TaskName -EA SilentlyContinue | Out-Null }
        else       { Disable-ScheduledTask -TaskPath $x.TaskPath -TaskName $x.TaskName -EA SilentlyContinue | Out-Null }
        Write-Host ("   {0} {1}" -f $(if ($Undo) {'+'} else {'-'}), $x.TaskName)
    }
}

# --- 8. Actualizador de Office --------------------------------------------
# 77 MB permanentes para buscar actualizaciones de Office. A Manual, no
# Deshabilitado: Office lo arranca solo cuando lo abris, asi que sigue
# actualizandose -- solo deja de estar sentado ahi el resto del dia.
#
# Deshabilitado del todo puede romper la reparacion de Office y, en algunas
# instalaciones, el propio arranque. Manual es el escalon correcto.
Write-Host '== actualizador de Office =='
if ($Undo) {
    Set-Service ClickToRunSvc -StartupType Automatic -EA SilentlyContinue
    Start-Service ClickToRunSvc -EA SilentlyContinue
    Write-Host '   ClickToRunSvc: Automatico, arrancado'
} else {
    Set-Service ClickToRunSvc -StartupType Manual -EA SilentlyContinue
    Stop-Service ClickToRunSvc -Force -EA SilentlyContinue
    Write-Host '   ClickToRunSvc: Manual, parado'
}

# --- 9. Servicio de diagnosticos (DPS) ------------------------------------
# 56 MB. Es lo que alimenta a los "solucionadores de problemas" de Windows --
# esos asistentes que casi nunca arreglan nada.
#
# CONTRAPARTIDA: los solucionadores dejan de funcionar, y con ellos el
# diagnostico automatico de red del icono de la bandeja. Diagnosticar a mano
# sigue igual.
Write-Host '== diagnosticos (DPS) =='
if ($Undo) {
    Set-Service DPS -StartupType Automatic -EA SilentlyContinue
    Start-Service DPS -EA SilentlyContinue
    Write-Host '   DPS: Automatico, arrancado'
} else {
    Stop-Service DPS -Force -EA SilentlyContinue
    Set-Service DPS -StartupType Disabled -EA SilentlyContinue
    Write-Host '   DPS: Deshabilitado, parado'
}

# --- 10. Flixmate -----------------------------------------------------------
# 56 MB de servicio en Auto para un DESCARGADOR DE VIDEO. Por dentro es yt-dlp,
# ffmpeg y deno empaquetados; firmado por Zinlab Technologies, instalado el
# 24/02/2026 en C:\Users\Public\AppData\Roaming -- que no es donde se instala el
# software normal, y por eso escapa al inventario por usuario.
#
# A Manual y no desinstalado: la app sigue funcionando cuando la abris, que es
# cuando la necesitas. Si resulta que no la usas nunca, su desinstalador esta en
# esa misma carpeta y libera ademas ~540 MB de disco.
Write-Host '== Flixmate (descargador de video) =='
if ($Undo) {
    Set-Service FlixmateService -StartupType Automatic -EA SilentlyContinue
    Start-Service FlixmateService -EA SilentlyContinue
    Write-Host '   FlixmateService: Automatico, arrancado'
} else {
    Stop-Service FlixmateService -Force -EA SilentlyContinue
    Set-Service FlixmateService -StartupType Manual -EA SilentlyContinue
    Write-Host '   FlixmateService: Manual, parado'
}

# --- 11. Panel de Widgets ---------------------------------------------------
# Estaba OCULTO de la barra de tareas (TaskbarDa = 0) y aun asi corria con SEIS
# procesos de WebView2, 56 MB. Esconderlo no lo apaga: sigue siendo una app web
# arrancada y viva. Lo unico que lo para es quitar el paquete.
#
# CONTRAPARTIDA: se va el panel de clima y noticias de Win+W. Reversible desde
# la Microsoft Store ("Widgets"), o con el -Undo de abajo si los archivos siguen
# en WindowsApps.
Write-Host '== panel de Widgets =='
if ($Undo) {
    $m = Get-AppxPackage -AllUsers -Name '*WebExperience*' -EA SilentlyContinue |
         Select-Object -First 1 -ExpandProperty InstallLocation
    if ($m -and (Test-Path "$m\AppXManifest.xml")) {
        Add-AppxPackage -DisableDevelopmentMode -Register "$m\AppXManifest.xml" -EA SilentlyContinue
        Write-Host '   Widgets: reinstalado desde WindowsApps'
    } else {
        Write-Host '   Widgets: los archivos ya no estan -- reinstalalo desde la Store'
    }
} else {
    Get-AppxPackage -Name '*WebExperience*' -EA SilentlyContinue | Remove-AppxPackage -EA SilentlyContinue
    Get-Process Widgets -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
    Write-Host '   Widgets: paquete WebExperience quitado'
}

# --- 12. Busqueda web del menu Inicio ---------------------------------------
# SearchHost.exe son 363 MB en seis procesos de WebView2, y la mayor parte es
# Bing y los "destacados" RENDERIZANDOSE dentro del menu Inicio.
#
# No se puede quitar SearchHost -- es la interfaz del Inicio y borrarla lo rompe.
# Lo que si se apaga es el contenido WEB, dejando la busqueda local intacta. Y
# aqui la local ya casi no se usa: para eso esta el launcher en Win+Space.
Write-Host '== busqueda web del menu Inicio =='
$claves = @(
    @{ k = 'HKCU:\Software\Policies\Microsoft\Windows\Explorer';        n = 'DisableSearchBoxSuggestions'; off = 1; on = 0 }
    @{ k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\SearchSettings'; n = 'IsDynamicSearchBoxEnabled'; off = 0; on = 1 }
    @{ k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Search';     n = 'BingSearchEnabled';          off = 0; on = 1 }
)
foreach ($c in $claves) {
    if (-not (Test-Path $c.k)) { New-Item -Path $c.k -Force | Out-Null }
    $v = if ($Undo) { $c.on } else { $c.off }
    Set-ItemProperty -Path $c.k -Name $c.n -Value $v -Type DWord -Force
    Write-Host ("   {0} = {1}" -f $c.n, $v)
}
# SearchHost relee al arrancar; matarlo lo hace volver limpio en el siguiente uso.
Get-Process SearchHost -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue
Write-Host '   SearchHost reiniciado'

Write-Host "`nhecho. Reinicia sesion para ver el efecto completo en el arranque."
