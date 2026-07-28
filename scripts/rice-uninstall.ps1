# Desinstalar aplicaciones desde la terminal, sin ir a Configuracion.
#
#   rice-uninstall.ps1            lista todo
#   rice-uninstall.ps1 discord    lista solo lo que coincida
#   rice-uninstall.ps1 -Size      ordena por tamano en disco, no por nombre
#
# De donde sale la lista, y por que de ahi: el registro. `winget list` tiene que
# parsearse de una tabla de ancho fijo que cambia con el idioma y con los nombres
# largos, y `Get-Package` se salta la mitad. Las claves Uninstall del registro
# las escribe todo instalador que se respete, estan en los dos hives y en las
# dos vistas (64 y 32 bits), y traen ya el comando exacto de desinstalacion.
#
# Nada se desinstala sin que lo confirmes escribiendo el numero y luego "s".
[CmdletBinding()]
param([string]$Filtro, [switch]$Size)

$ErrorActionPreference = 'SilentlyContinue'

# --- scoop -----------------------------------------------------------------
# Aparte porque scoop no toca el registro: sus aplicaciones son directorios.
$apps = @()
$scoopDir = "$env:USERPROFILE\scoop\apps"
if (Test-Path $scoopDir) {
    foreach ($d in Get-ChildItem $scoopDir -Directory) {
        if ($d.Name -eq 'scoop') { continue }
        $apps += [pscustomobject]@{
            Nombre    = $d.Name
            Editor    = 'scoop'
            MB        = 0
            Origen    = 'scoop'
            Comando   = "scoop uninstall $($d.Name)"
        }
    }
}

# --- registro --------------------------------------------------------------
$keys = @(
    'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'
)
foreach ($k in $keys) {
    foreach ($e in Get-ItemProperty $k) {
        if (-not $e.DisplayName) { continue }
        # Las actualizaciones y los componentes del sistema no son aplicaciones
        # que nadie quiera desinstalar desde aqui.
        if ($e.SystemComponent -eq 1) { continue }
        if ($e.ReleaseType -in 'Security Update', 'Update Rollup', 'Hotfix') { continue }
        if (-not $e.UninstallString) { continue }
        $apps += [pscustomobject]@{
            Nombre  = $e.DisplayName
            Editor  = if ($e.Publisher) { $e.Publisher } else { '-' }
            MB      = if ($e.EstimatedSize) { [int]($e.EstimatedSize / 1024) } else { 0 }
            Origen  = if ($k -like 'HKCU*') { 'usuario' } else { 'equipo' }
            Comando = $e.UninstallString
        }
    }
}

$apps = $apps | Sort-Object Nombre -Unique
if ($Filtro) { $apps = $apps | Where-Object { $_.Nombre -like "*$Filtro*" -or $_.Editor -like "*$Filtro*" } }
$apps = if ($Size) { $apps | Sort-Object MB -Descending } else { $apps | Sort-Object Nombre }

if (-not $apps) { Write-Host "nada coincide con '$Filtro'." -ForegroundColor Yellow; return }

$i = 0
$apps | ForEach-Object {
    $i++
    $tam = if ($_.MB -gt 0) { '{0,6:N0} MB' -f $_.MB } else { '          ' }
    Write-Host ("{0,3}. {1,-52} {2} {3,-8} {4}" -f $i, `
        $(if ($_.Nombre.Length -gt 52) { $_.Nombre.Substring(0, 49) + '...' } else { $_.Nombre }), `
        $tam, $_.Origen, $_.Editor)
}
Write-Host ""
$sel = Read-Host "numero a desinstalar (Enter para salir)"
if (-not $sel) { return }
$n = 0
if (-not [int]::TryParse($sel, [ref]$n) -or $n -lt 1 -or $n -gt $apps.Count) {
    Write-Host 'numero fuera de rango.' -ForegroundColor Yellow; return
}
$app = $apps[$n - 1]

Write-Host ""
Write-Host ("  aplicacion: {0}" -f $app.Nombre) -ForegroundColor Cyan
Write-Host ("  editor:     {0}" -f $app.Editor)
Write-Host ("  se ejecuta: {0}" -f $app.Comando) -ForegroundColor Yellow
if ($app.Origen -eq 'equipo') { Write-Host '  (instalada para todo el equipo: pedira permisos de administrador)' }
Write-Host ""
if ((Read-Host 'seguro? (s/N)') -notmatch '^[sSyY]') { Write-Host 'cancelado.'; return }

if ($app.Origen -eq 'scoop') {
    & scoop uninstall $app.Nombre
    return
}
# El UninstallString viene tal cual lo dejo el instalador: unas veces con el
# ejecutable entrecomillado y argumentos detras, otras sin comillas. Se parte
# respetando las comillas y se lanza sin pasar por cmd, para que un nombre con
# espacios no se rompa por el camino.
$cmd = $app.Comando.Trim()
if ($cmd.StartsWith('"')) {
    $end = $cmd.IndexOf('"', 1)
    $exe = $cmd.Substring(1, $end - 1)
    $arg = $cmd.Substring($end + 1).Trim()
} else {
    $sp = $cmd.IndexOf(' ')
    if ($sp -lt 0) { $exe = $cmd; $arg = '' }
    else { $exe = $cmd.Substring(0, $sp); $arg = $cmd.Substring($sp + 1).Trim() }
}
Write-Host "lanzando el desinstalador..."
if ($arg) { Start-Process -FilePath $exe -ArgumentList $arg -Wait }
else { Start-Process -FilePath $exe -Wait }
Write-Host 'terminado.'
