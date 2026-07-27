<#
  Quita del autoarranque las aplicaciones que se abren perfectamente a mano.

      rice-trim-run.ps1            quita (guarda copia antes)
      rice-trim-run.ps1 -Restore   devuelve exactamente lo que habia
      rice-trim-run.ps1 -List      solo enseña que hay y que se quitaria

  Sin admin: todo vive en HKCU.

  La copia guarda nombre Y valor, asi que -Restore reconstruye la linea de
  comandos original. Sin eso, "restaurar" seria adivinar los argumentos, que en
  varias de estas son los que las hacen arrancar silenciosas (-silent, --minimized,
  --autostarted...).
#>
param([switch]$Restore, [switch]$List)

$Run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$Bak = "$env:USERPROFILE\.config\.run-trimmed.json"

# Nombres exactos de las entradas, no patrones: un patron como "steam" tambien
# cazaria cosas que no queremos tocar.
$targets = @(
    # Lanzadores: cada uno se abre solo al lanzar un juego desde su acceso directo.
    # Vanguard (el anti-cheat) es un SERVICIO aparte y no se toca aqui.
    'Steam', 'Battle.net', 'EpicGamesLauncher', 'EADM', 'RiotClient'
    # Chat y productividad.
    'com.squirrel.slack.slack', 'com.squirrel.Teams.Teams', 'electron.app.Notion'
    'electron.app.Untapped.gg Companion', 'Figma Agent', 'Microsoft.Lists'
    # Navegadores: los dos arrancaban solos.
    'MicrosoftEdgeAutoLaunch_0A6413D02CDC4E4C94BA23B5FAD3E081'
    'Mozilla-Firefox-CA9422711AE1A81C'
    # Utilidades y hardware.
    'YouTube Downloader', 'UA Connect', 'LGHUB', 'NVIDIA Broadcast ', 'OneDrive'
)

if ($List) {
    $p = Get-ItemProperty $Run
    foreach ($n in (Get-Item $Run).Property | Sort-Object) {
        $mark = if ($targets -contains $n) { 'QUITAR ' } else { 'quedarse' }
        Write-Host ("  {0}  {1}" -f $mark, $n)
    }
    return
}

if ($Restore) {
    if (-not (Test-Path $Bak)) { Write-Host 'No hay copia que restaurar.'; return }
    $saved = Get-Content $Bak -Raw | ConvertFrom-Json
    foreach ($e in $saved.PSObject.Properties) {
        New-ItemProperty $Run -Name $e.Name -Value $e.Value -PropertyType String -Force | Out-Null
        Write-Host ("  restaurado  {0}" -f $e.Name)
    }
    Write-Host "`n$($saved.PSObject.Properties.Count) entrada(s) devueltas."
    return
}

$p = Get-ItemProperty $Run
$saved = @{}
$n = 0
foreach ($name in (Get-Item $Run).Property) {
    if ($targets -notcontains $name) { continue }
    $saved[$name] = $p.$name
    Remove-ItemProperty $Run -Name $name -ErrorAction SilentlyContinue
    Write-Host ("  quitado  {0}" -f $name)
    $n++
}
if ($n) {
    # Fusionar con cualquier copia previa, para no perder una tanda anterior.
    if (Test-Path $Bak) {
        (Get-Content $Bak -Raw | ConvertFrom-Json).PSObject.Properties |
          ForEach-Object { if (-not $saved.ContainsKey($_.Name)) { $saved[$_.Name] = $_.Value } }
    }
    $saved | ConvertTo-Json | Set-Content $Bak -Encoding utf8
}
Write-Host "`n$n entrada(s) quitadas. Copia en $Bak -- '-Restore' las devuelve con sus argumentos."
Write-Host "Quedan $((Get-Item $Run).Property.Count) en el autoarranque."
