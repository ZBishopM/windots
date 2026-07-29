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

Write-Host "`nhecho. Reinicia sesion para ver el efecto completo en el arranque."
