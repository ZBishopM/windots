# Vigila si el PR #1392 de GlazeWM (animaciones de ventana y de cambio de
# workspace) ha mergeado, para poder cambiar nuestro ws-slide casero por el
# oficial. Avisa una sola vez y no vuelve a molestar.
#
# SEMANAL, no en cada inicio de sesion. Es un PR que lleva meses abierto: pedirlo
# a la API de GitHub en cada login era una llamada de red por algo que no cambia
# de un dia para otro. El script se sigue lanzando en cada arranque, pero seis de
# cada siete veces sale por la puerta de abajo sin tocar la red.
$flag  = "$env:USERPROFILE\.config\.glaze-anim-merged"   # ya aviso: no repetir nunca
$stamp = "$env:USERPROFILE\.config\.glaze-anim-checked"  # cuando se miro por ultima vez

if (Test-Path $flag) { return }

if (Test-Path $stamp) {
    $desde = (Get-Date) - (Get-Item $stamp).LastWriteTime
    if ($desde -lt [TimeSpan]::FromDays(7)) { return }
}

try {
    $pr = Invoke-RestMethod -Uri 'https://api.github.com/repos/glzr-io/glazewm/pulls/1392' `
        -Headers @{ 'User-Agent' = 'rice-animcheck' } -TimeoutSec 8
} catch { return }   # sin red o con limite de peticiones -> NO se sella la fecha,
                     # asi se reintenta en el siguiente inicio en vez de perder la semana

# Sellar solo tras una consulta que si respondio.
Set-Content $stamp (Get-Date -Format 'o') -Encoding utf8

if ($pr.merged -eq $true) {
    New-Item -ItemType File -Force $flag | Out-Null

    # UN solo aviso. Antes escribia ademas island.json, asi que el mismo hecho
    # salia dos veces -- toast e isla -- y encima con textos distintos.
    $notify = "$env:USERPROFILE\dev\target\release\shadowplay-notify.exe"
    if (Test-Path $notify) {
        Start-Process $notify -ArgumentList "--title `"GlazeWM #1392 mergeado`" --body `"Animaciones oficiales - actualiza GlazeWM y activa el slide`" --icon check --accent `"#a9b56a`" --hold 12"
    }
}
