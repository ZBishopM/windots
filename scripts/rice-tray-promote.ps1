# Saca de la bandeja "oculta" los iconos que estan en uso, para que aparezcan en
# nuestra barra.
#
# Windows 11 divide la bandeja en dos: los promocionados, que se ven en la franja,
# y el resto, que viven detras del boton "Mostrar iconos ocultos". Nuestra barra
# solo puede leer los primeros: los otros no existen en el arbol de UI Automation
# hasta que se abre el desplegable, y abrirlo cada dos segundos seria un menu
# parpadeando en la esquina para siempre.
#
# Normalmente esto se arregla arrastrando el icono fuera del desplegable. Aqui no:
# la barra de tareas esta invisible a proposito (ver crates/taskbar/src/tray.rs),
# asi que no hay nada que arrastrar. De ahi este script.
#
# La lista vive en HKCU\Control Panel\NotifyIconSettings, una clave por icono que
# se haya registrado alguna vez -- 134 en esta maquina, la mayoria de programas
# desinstalados hace anos. Por eso NO se promociona todo: solo aquello cuyo
# ejecutable existe y ademas esta corriendo ahora mismo. Promocionar el resto
# llenaria la barra de huecos de cosas muertas.
#
#   rice-tray-promote.ps1            promociona lo que este corriendo
#   rice-tray-promote.ps1 -All       promociona todo lo que exista en disco
#   rice-tray-promote.ps1 -Reset     devuelve todo al desplegable
[CmdletBinding()]
param(
    [switch]$All,
    [switch]$Reset
)

$key = 'HKCU:\Control Panel\NotifyIconSettings'
if (-not (Test-Path $key)) { Write-Host 'No existe NotifyIconSettings.'; return }

# Las rutas se guardan con el GUID de la carpeta conocida por delante. Estos son
# los que aparecen en la practica; cualquier otro se deja tal cual y el archivo
# simplemente no se encontrara, que es el fallo correcto.
$folders = @{
    '{6D809377-6AF0-444B-8957-A3773F02200E}' = ${env:ProgramFiles}
    '{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}' = ${env:ProgramFiles(x86)}
    '{F38BF404-1D43-42F2-9305-67DE0B28FC23}' = $env:SystemRoot
    '{B4BFCC3A-DB2C-424C-B029-7FE99A87C641}' = "$env:USERPROFILE\Desktop"
    '{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}' = $env:LOCALAPPDATA
    '{3EB685DB-65F9-4CF6-A03A-E3EF65729F3D}' = $env:APPDATA
}
function Resolve-TrayPath([string]$p) {
    if (-not $p) { return $null }
    foreach ($g in $folders.Keys) {
        if ($p.StartsWith($g)) { return ($folders[$g] + $p.Substring($g.Length)) }
    }
    return $p
}

$changed = 0
foreach ($sub in Get-ChildItem $key) {
    $props = Get-ItemProperty $sub.PSPath
    $want = 0
    if (-not $Reset) {
        $exe = Resolve-TrayPath $props.ExecutablePath
        if ($exe -and (Test-Path -LiteralPath $exe)) {
            if ($All) {
                $want = 1
            } else {
                $stem = [IO.Path]::GetFileNameWithoutExtension($exe)
                if (Get-Process -Name $stem -ErrorAction SilentlyContinue) { $want = 1 }
            }
        }
    }
    $have = [int]($props.IsPromoted)
    if ($have -ne $want) {
        Set-ItemProperty $sub.PSPath -Name IsPromoted -Value $want -Type DWord
        $changed++
        $tip = $props.InitialTooltip
        if (-not $tip) { $tip = Split-Path (Resolve-TrayPath $props.ExecutablePath) -Leaf }
        Write-Host ("  {0} {1}" -f $(if ($want) { '+' } else { '-' }), $tip)
    }
}
Write-Host ("{0} icono(s) cambiado(s). La barra lo recoge en dos segundos." -f $changed)
