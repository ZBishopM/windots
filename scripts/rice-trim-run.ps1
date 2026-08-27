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

  Ahora lo lleva lib\rice-undo.ps1, y eso arregla un fallo doble que tenia la
  copia a mano: Get-ItemProperty EXPANDE las variables de entorno al leer, y el
  -Restore reescribia siempre como String. Una entrada REG_EXPAND_SZ que valia
  '%ProgramFiles%\X\x.exe' se guardaba ya expandida y volvia como ruta fija de
  tipo String. Funcionaba... hasta que cambiaras algo de sitio.
#>
param([switch]$Restore, [switch]$List)

. "$env:USERPROFILE\.config\lib\rice-undo.ps1"

$Run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$Ambito = 'trim-run'
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
    Undo-Rice $Ambito
    # La copia vieja se queda por si hay que mirarla, pero ya no manda.
    if (Test-Path $Bak) { Write-Host "(el antiguo $Bak sigue ahi; ya no se usa)" }
    return
}

$n = 0
# Sin -Nuevo: si ya quitaste una tanda antes y no la has devuelto, esta se suma
# en vez de pisar el undo anterior.
Start-RiceUndo $Ambito
try {
    foreach ($name in (Get-Item $Run).Property) {
        if ($targets -notcontains $name) { continue }
        if (Remove-RegValueTracked $Run $name) {
            Write-Host ("  quitado  {0}" -f $name)
            $n++
        }
    }
} finally { Save-RiceUndo }

Write-Host "`n$n entrada(s) quitadas. '-Restore' las devuelve con sus argumentos y su tipo original."
Write-Host "Quedan $((Get-Item $Run).Property.Count) en el autoarranque."
