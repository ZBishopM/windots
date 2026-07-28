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
# quedarse corriendo. No se borran: se mueven a un subdirectorio, que Windows no
# recorre, asi que devolverlos es arrastrarlos de vuelta.
#
# Cloudflare WARP NO esta en la lista a proposito: lo pediste explicitamente.
Write-Host '== arranques muertos (todos los usuarios) =='
$sf   = "$env:ProgramData\Microsoft\Windows\Start Menu\Programs\Startup"
$off  = Join-Path $sf 'desactivado-por-rice'
$dead = @('CodeMeter Control Center.lnk', 'ScpToolkit Tray Notifications.lnk')
if (-not (Test-Path $off)) { New-Item $off -ItemType Directory -Force | Out-Null }
foreach ($n in $dead) {
    if ($Undo) {
        $src = Join-Path $off $n
        if (Test-Path $src) { Move-Item $src (Join-Path $sf $n) -Force; Write-Host "   + $n" }
    } else {
        $src = Join-Path $sf $n
        if (Test-Path $src) { Move-Item $src (Join-Path $off $n) -Force; Write-Host "   - $n" }
    }
}

Write-Host "`nhecho. Reinicia sesion para ver el efecto completo en el arranque."
