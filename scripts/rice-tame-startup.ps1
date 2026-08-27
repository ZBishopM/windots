#Requires -Version 7
<#
  Silencia los actualizadores que interrumpen y baja a demanda los servicios que
  no hacen falta en cada arranque.

      pwsh -File rice-tame-startup.ps1            aplica
      pwsh -File rice-tame-startup.ps1 -Restore   deshace

  NECESITA ADMIN. Todo lo que toca vive en HKLM o en el programador de tareas a
  nivel de máquina, así que no hay forma de hacerlo como usuario.

  Nada de esto desinstala nada: las tareas quedan desactivadas (no borradas) y
  los servicios pasan a Manual (no Deshabilitado), así que la aplicación que los
  necesite puede seguir arrancándolos ella misma cuando la abras.

  El deshacer lo lleva lib\rice-undo.ps1. Antes -Restore ponia TODOS los
  servicios en Automatic y activaba TODAS las tareas, sin mirar como estaban.
  Si un servicio ya venia en Manual de fabrica, "restaurar" te lo dejaba
  arrancando en cada inicio: peor de lo que estaba.
#>
param([switch]$Restore)

. "$env:USERPROFILE\.config\lib\rice-undo.ps1"
$Ambito = 'tame-startup'

$ErrorActionPreference = 'Continue'
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
        ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host 'Hay que ejecutarlo como administrador.' -ForegroundColor Red
    exit 1
}

# Tareas de actualización que se disparan solas y sacan ventanas. Las tres de
# Nefarius son el stack de mandos (ScpToolkit / HidHide / ViGEmBus): comprobado,
# `updater` corrió a las 10:00 del día en que se reportó la molestia.
$tasks = @(
    @{ Name = 'updater';           Path = '\'; Why = 'ScpToolkit' }
    @{ Name = 'HidHide_Updater';   Path = '\'; Why = 'HidHide (Nefarius)' }
    @{ Name = 'ViGEmBus_Updater';  Path = '\'; Why = 'ViGEmBus (Nefarius)' }
)

# Servicios que sólo hacen falta cuando usas la aplicación correspondiente.
# Manual, NO deshabilitado: la app los arranca al abrirse.
$services = @(
    @{ Name = 'OVRService'; Why = 'Oculus / Meta Horizon Link - ~286 MB entre sus 3 procesos' }
    @{ Name = 'MySQL80';    Why = 'MySQL - ~427 MB, arrancalo cuando desarrolles' }

    # Servicios de actualizacion que arrancan con Windows y no hacen nada mas.
    # Cada aplicacion sigue pudiendo buscar actualizaciones cuando la abres; lo
    # que se quita es que un demonio residente lo haga por su cuenta.
    @{ Name = 'EaseUS UPDATE SERVICE';                   Why = 'actualizador de EaseUS' }
    @{ Name = 'Flixmate.UpdateService';                  Why = 'actualizador de Flixmate' }
    @{ Name = 'LGHUBUpdaterService';                     Why = 'actualizador de LG Hub' }
    @{ Name = 'GoogleUpdaterService152.0.7933.0';        Why = 'actualizador de Google' }
    @{ Name = 'GoogleUpdaterInternalService152.0.7933.0'; Why = 'actualizador de Google (interno)' }
    @{ Name = 'gupdate';                                 Why = 'Google Update (heredado)' }
    @{ Name = 'edgeupdate';                              Why = 'actualizador de Edge' }
    @{ Name = 'Bonjour Service';                         Why = 'descubrimiento de red de Apple' }
    @{ Name = 'CodeMeter.exe';                           Why = 'licencias Wibu - OJO si usas software CAD/DAW que lo pida' }

    # Encontrados auditando la maquina con CPU medida por muestreo, no a ojo.
    #
    # PACE es el licenciador de iLok. Se comprobo la lista de programas
    # instalados: no hay Pro Tools, ni Waves, ni Cubase, ni UAD, ni nada que lo
    # pida. Es un huerfano de algo que se desinstalo y dejo el servicio puesto.
    @{ Name = 'PaceLicenseDServices'; Why = 'licencias iLok/PACE - 122 MB, y no hay software instalado que lo use' }

    # Tres tuneles arrancando con Windows a la vez, y solo UNO tiene adaptador
    # levantado: Tailscale. Los otros dos estan residentes sin hacer nada.
    # Manual, no deshabilitado: al abrir la aplicacion de WARP el servicio
    # arranca solo, asi que no pierdes nada salvo que arranque en cada inicio.
    @{ Name = 'CloudflareWARP'; Why = 'Cloudflare WARP - 48 MB, sin adaptador activo (el que usas es Tailscale)' }
    @{ Name = 'Cloudflared';    Why = 'agente cloudflared - 39 MB, idem' }
)

if ($Restore) {
    Undo-Rice $Ambito
    Write-Host "`nListo. Los servicios vuelven a su tipo de inicio original, no a Automatic."
    return
}

Start-RiceUndo $Ambito
try {
    Write-Host "=== tareas ==="
    foreach ($t in $tasks) {
        $obj = Get-ScheduledTask -TaskName $t.Name -TaskPath $t.Path -ErrorAction SilentlyContinue
        if (-not $obj) { Write-Host ("  {0,-18} no existe" -f $t.Name); continue }
        try {
            # Se apunta como estaba ANTES. Una tarea que ya venia desactivada no
            # debe quedar activada al deshacer.
            Register-RiceUndo -Tipo tarea -Datos @{
                nombre   = $t.Name
                ruta     = $t.Path
                activada = ($obj.State -ne 'Disabled')
            }
            Disable-ScheduledTask -TaskName $t.Name -TaskPath $t.Path -EA Stop | Out-Null
            $s = (Get-ScheduledTask -TaskName $t.Name -TaskPath $t.Path).State
            Write-Host ("  {0,-18} {1,-9} ({2})" -f $t.Name, $s, $t.Why)
        } catch { Write-Host ("  {0,-18} FALLO: {1}" -f $t.Name, $_.Exception.Message) }
    }

    Write-Host "`n=== servicios ==="
    foreach ($s in $services) {
        $svc = Get-Service $s.Name -ErrorAction SilentlyContinue
        if (-not $svc) { Write-Host ("  {0,-14} no existe" -f $s.Name); continue }
        try {
            $antes = $svc.StartType
            if ($antes -eq 'Manual') {
                Write-Host ("  {0,-14} ya estaba en Manual, no se toca" -f $s.Name)
                continue
            }
            Register-RiceUndo -Tipo servicio -Datos @{ nombre = $s.Name; inicio = "$antes" }
            Set-Service $s.Name -StartupType Manual -EA Stop
            Stop-Service $s.Name -Force -EA SilentlyContinue
            Write-Host ("  {0,-14} {1} -> {2}/{3}  ({4})" -f $s.Name, $antes,
                (Get-Service $s.Name).StartType, (Get-Service $s.Name).Status, $s.Why)
        } catch { Write-Host ("  {0,-14} FALLO: {1}" -f $s.Name, $_.Exception.Message) }
    }
} finally { Save-RiceUndo }

Write-Host "`nListo. Nada se ha desinstalado: -Restore lo devuelve todo."
