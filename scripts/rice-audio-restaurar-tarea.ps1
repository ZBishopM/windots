<#
  Registra (o quita) la tarea que restaura el audio al volver de hibernar.

      rice-audio-restaurar-tarea.ps1           registra
      rice-audio-restaurar-tarea.ps1 -Quitar   la borra

  El disparador es un EVENTO, no un temporizador: el registro System, origen
  Microsoft-Windows-Power-Troubleshooter, ID 1, que Windows emite al despertar
  de suspension o hibernacion. Nada de sondear cada N minutos preguntando si
  ya despertamos.

  El retraso de 8 s no es un margen inventado: al emitirse ese evento Windows
  todavia esta reenumerando los endpoints de audio, y fijar el predeterminado
  antes de que aparezcan todos no sirve de nada. Se espera a que la lista este
  completa y recien ahi se corrige.

  Corre como el usuario actual y sin privilegios elevados: cambiar el
  dispositivo predeterminado es una preferencia por usuario, no del sistema.
#>
param([switch]$Quitar)

$ErrorActionPreference = 'Stop'
$nombre = 'rice-audio-restaurar'
$script = Join-Path $env:USERPROFILE '.config\rice-audio-restaurar.ps1'

if ($Quitar) {
    if (Get-ScheduledTask -TaskName $nombre -EA SilentlyContinue) {
        Unregister-ScheduledTask -TaskName $nombre -Confirm:$false
        Write-Host "tarea '$nombre' eliminada"
    } else {
        Write-Host "no existe la tarea '$nombre', no hay nada que quitar"
    }
    return
}

if (-not (Test-Path $script)) { throw "no encuentro $script" }

$accion = New-ScheduledTaskAction -Execute 'powershell.exe' `
    -Argument "-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File `"$script`""

# El modulo ScheduledTasks no expone "al reanudar", asi que el disparador se
# arma como suscripcion XML al log de eventos. Es el mismo mecanismo que usa
# la interfaz del Programador de tareas para "Al producirse un evento".
$xmlEvento = @'
<QueryList><Query Id="0" Path="System"><Select Path="System">*[System[Provider[@Name='Microsoft-Windows-Power-Troubleshooter'] and EventID=1]]</Select></Query></QueryList>
'@
$disparador = New-CimInstance -CimClass (Get-CimClass -ClassName MSFT_TaskEventTrigger `
                -Namespace Root/Microsoft/Windows/TaskScheduler) -ClientOnly
$disparador.Enabled      = $true
$disparador.Subscription = $xmlEvento
$disparador.Delay        = 'PT8S'

$ajustes = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries `
    -StartWhenAvailable -ExecutionTimeLimit (New-TimeSpan -Minutes 2)

Register-ScheduledTask -TaskName $nombre -Action $accion -Trigger $disparador `
    -Settings $ajustes -Description 'Restaura microfono y salida de audio al volver de hibernar (ver rice-audio-restaurar.ps1)' -Force | Out-Null

Write-Host "tarea '$nombre' registrada"
Write-Host "  disparador: evento Power-Troubleshooter ID 1 (despertar), +8 s"
Write-Host "  probar sin hibernar:  Start-ScheduledTask -TaskName $nombre"
Write-Host "  ver que hizo:         Get-Content `"$env:USERPROFILE\.config\audio-restaurar.log`" -Tail 10"
