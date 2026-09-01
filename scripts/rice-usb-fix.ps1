# Recupera un dispositivo USB atascado sin desenchufarlo, y quita lo que hace
# que se atasque.
#
#   rice-usb-fix.ps1              busca dispositivos en error y los reinicia
#   rice-usb-fix.ps1 -Prevenir    ademas desactiva la suspension selectiva
#   rice-usb-fix.ps1 -Solo        solo diagnostica, no toca nada
#
# NECESITA ADMINISTRADOR: reiniciar un nodo PnP y escribir en el plan de energia
# son operaciones de sistema.
#
# EL CASO QUE LO MOTIVO, para que se entienda que busca:
#
# El Blue Snowball aparecia como 'Dispositivo USB desconocido (Error de solicitud
# de descriptor de dispositivo)', VID_0000&PID_0002, codigo de problema 43. Ese
# VID/PID no es del fabricante: es el marcador que pone Windows cuando NO CONSIGUE
# LEER el descriptor, o sea que el puerto ve algo conectado pero el saludo USB
# inicial falla y nunca llega a saber que es un 0D8C:0005. Por eso el micro no
# salia ni en Discord ni en Sonido: para el sistema no existe.
#
# Estaba en Port_#0006.Hub_#0002, colgando de un concentrador raiz USB 3.0. El
# Snowball es full-speed (USB 1.1), y los xHCI de Intel fallan al enumerar
# dispositivos full-speed en puertos 3.x mas a menudo de lo que deberian. Se
# limpia quitando la alimentacion del puerto -- que es justo lo que hace
# desenchufarlo, y lo que este script hace por software.
#
# SI ESTO SE REPITE, la solucion definitiva no es un script: pasar el micro a un
# puerto USB 2.0 (los negros, no los azules) o a un hub con alimentacion propia.
[CmdletBinding()]
param([switch]$Prevenir, [switch]$Solo)

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Write-Host ("administrador: {0}" -f $admin)
if (-not $admin -and -not $Solo) { Write-Host 'Hace falta administrador (o usa -Solo para ver el diagnostico).' -ForegroundColor Yellow; exit 1 }

# --- 1. que esta en error ---------------------------------------------------
Write-Host "`n== dispositivos USB en error =="
$malos = Get-PnpDevice -PresentOnly -EA SilentlyContinue |
         Where-Object { $_.Status -ne 'OK' -and $_.InstanceId -like 'USB\*' }
if (-not $malos) { Write-Host '   ninguno.' -ForegroundColor Green }
foreach ($m in $malos) {
    $prob = (Get-PnpDeviceProperty -InstanceId $m.InstanceId -KeyName 'DEVPKEY_Device_ProblemCode' -EA SilentlyContinue).Data
    $loc  = (Get-PnpDeviceProperty -InstanceId $m.InstanceId -KeyName 'DEVPKEY_Device_LocationInfo' -EA SilentlyContinue).Data
    Write-Host ("   [{0}] {1}" -f $prob, $m.FriendlyName)
    Write-Host ("        {0}   {1}" -f $loc, $m.InstanceId)
}
if ($Solo) { Write-Host "`n(-Solo: no se toca nada)"; exit 0 }

# --- 2. reiniciarlos, de menos a mas invasivo -------------------------------
# Primero el propio nodo. Si no basta, su concentrador padre, que es lo que de
# verdad recicla la alimentacion del puerto. Reiniciar el concentrador
# desconecta un instante TODO lo que cuelgue de el -- raton incluido -- asi que
# no se hace de entrada.
foreach ($m in $malos) {
    Write-Host ("`n== reinicio {0} ==" -f $m.FriendlyName)
    & pnputil /restart-device "$($m.InstanceId)" 2>&1 | ForEach-Object { Write-Host "   $_" }
    Start-Sleep -Seconds 3
    $ahora = Get-PnpDevice -InstanceId $m.InstanceId -EA SilentlyContinue
    if (-not $ahora -or $ahora.Status -eq 'OK') { Write-Host '   resuelto.' -ForegroundColor Green; continue }

    $padre = (Get-PnpDeviceProperty -InstanceId $m.InstanceId -KeyName 'DEVPKEY_Device_Parent' -EA SilentlyContinue).Data
    if (-not $padre) { Write-Host '   sigue en error y no encuentro su concentrador.'; continue }
    Write-Host ("   sigue en error; reinicio el concentrador padre: {0}" -f $padre)
    Write-Host '   (todo lo que cuelgue de el parpadea un segundo)'
    & pnputil /restart-device "$padre" 2>&1 | ForEach-Object { Write-Host "   $_" }
    Start-Sleep -Seconds 6
    $ahora = Get-PnpDevice -InstanceId $m.InstanceId -EA SilentlyContinue
    if (-not $ahora -or $ahora.Status -eq 'OK') { Write-Host '   resuelto.' -ForegroundColor Green }
    else { Write-Host '   NO se recupero por software: hay que desenchufar y volver a enchufar.' -ForegroundColor Yellow }
}

# --- 3. prevenir ------------------------------------------------------------
if ($Prevenir) {
    Write-Host "`n== prevencion =="
    # La suspension selectiva estaba ACTIVADA incluso en el plan 'Maximo
    # rendimiento', que es contraintuitivo y por eso pasa desapercibido.
    powercfg /setacvalueindex SCHEME_CURRENT 2a737441-1930-4402-8d77-b2bebba308a3 48e6b7a6-50f5-4782-a5d4-53bb8f07e226 0
    powercfg /setdcvalueindex SCHEME_CURRENT 2a737441-1930-4402-8d77-b2bebba308a3 48e6b7a6-50f5-4782-a5d4-53bb8f07e226 0
    powercfg /setactive SCHEME_CURRENT
    Write-Host '   suspension selectiva de USB: desactivada (CA y CC)'

    # "Permitir que el equipo apague este dispositivo para ahorrar energia" en
    # los concentradores. Es la otra mitad: el plan de energia lo permite, y cada
    # nodo tiene ademas su propia casilla.
    $n = 0
    Get-CimInstance -Namespace root\wmi -ClassName MSPower_DeviceEnable -EA SilentlyContinue | ForEach-Object {
        $id = $_.InstanceName -replace '_0$', ''
        $d = Get-PnpDevice -InstanceId $id -EA SilentlyContinue
        if ($d -and $d.Class -eq 'USB' -and $_.Enable) {
            $_.Enable = $false
            try { Set-CimInstance -InputObject $_ -ErrorAction Stop; $n++; Write-Host ("   ya no puede apagarse: {0}" -f $d.FriendlyName) }
            catch { Write-Host ("   no pude cambiar {0}: {1}" -f $d.FriendlyName, $_.Exception.Message) }
        }
    }
    Write-Host ("   {0} nodos USB ajustados" -f $n)
    Write-Host '   revertir: mismas ordenes con 1 en vez de 0, y la casilla de vuelta en Administrador de dispositivos'
}

Write-Host "`n== estado final =="
Get-PnpDevice -PresentOnly -EA SilentlyContinue | Where-Object { $_.InstanceId -like 'USB\*' -and $_.Status -ne 'OK' } |
    ForEach-Object { Write-Host ("   sigue en error: {0}" -f $_.FriendlyName) }
Get-PnpDevice -EA SilentlyContinue | Where-Object { $_.InstanceId -like 'USB\VID_0D8C&PID_0005*' } |
    ForEach-Object { Write-Host ("   Snowball: Present={0} Status={1}" -f $_.Present, $_.Status) }
