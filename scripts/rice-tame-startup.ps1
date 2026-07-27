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
#>
param([switch]$Restore)

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
)

Write-Host "=== tareas ==="
foreach ($t in $tasks) {
    $obj = Get-ScheduledTask -TaskName $t.Name -TaskPath $t.Path -ErrorAction SilentlyContinue
    if (-not $obj) { Write-Host ("  {0,-18} no existe" -f $t.Name); continue }
    try {
        if ($Restore) { Enable-ScheduledTask  -TaskName $t.Name -TaskPath $t.Path -EA Stop | Out-Null }
        else          { Disable-ScheduledTask -TaskName $t.Name -TaskPath $t.Path -EA Stop | Out-Null }
        $s = (Get-ScheduledTask -TaskName $t.Name -TaskPath $t.Path).State
        Write-Host ("  {0,-18} {1,-9} ({2})" -f $t.Name, $s, $t.Why)
    } catch { Write-Host ("  {0,-18} FALLO: {1}" -f $t.Name, $_.Exception.Message) }
}

Write-Host "`n=== servicios ==="
foreach ($s in $services) {
    $svc = Get-Service $s.Name -ErrorAction SilentlyContinue
    if (-not $svc) { Write-Host ("  {0,-14} no existe" -f $s.Name); continue }
    try {
        if ($Restore) {
            Set-Service $s.Name -StartupType Automatic -EA Stop
            Start-Service $s.Name -EA SilentlyContinue
        } else {
            Set-Service $s.Name -StartupType Manual -EA Stop
            Stop-Service $s.Name -Force -EA SilentlyContinue
        }
        $svc.Refresh()
        Write-Host ("  {0,-14} {1}/{2}  ({3})" -f $s.Name, (Get-Service $s.Name).StartType, (Get-Service $s.Name).Status, $s.Why)
    } catch { Write-Host ("  {0,-14} FALLO: {1}" -f $s.Name, $_.Exception.Message) }
}

Write-Host "`nListo. Nada se ha desinstalado: -Restore lo devuelve todo."
