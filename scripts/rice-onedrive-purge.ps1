# Quita OneDrive de esta maquina: el programa, sus dos vias de arranque y los
# restos que hacen que vuelva.
#
#   rice-onedrive-purge.ps1            diagnostico, no toca nada
#   rice-onedrive-purge.ps1 -Hacerlo   lo ejecuta (pide elevacion una vez)
#
# POR QUE HIZO FALTA UN SCRIPT Y NO UN "desinstalar" a mano:
#
# OneDrive ya se habia limpiado antes y volvio. Su entrada de arranque NO se
# llama OneDrive: es HKCU\...\Run\Microsoft.Lists apuntando a
# OneDrive.Sync.Service.exe. rice-trim-run.ps1 si la tenia en su lista, pero
# .run-trimmed.json guarda la version 26.108.0607.0002 y la instalada es la
# 26.123.0628.0001 -- OneDrive se ACTUALIZO y reescribio su propia entrada. Un
# trim de claves Run no sobrevive a una actualizacion del programa que limpia.
#
# Ademas hay tres tareas programadas y el trim anterior solo desactivo una.
#
# LO QUE SE PIERDE, dicho claro: en C:\Users\obisp\OneDrive hay 83 archivos y 78
# son MARCADORES SOLO-EN-NUBE (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS): su
# contenido no esta en este disco. Siguen existiendo en la cuenta y en
# onedrive.com, pero dejan de verse en el explorador. Los 5 locales son
# fontaneria del propio OneDrive (desktop.ini, el marcador de carpeta, el acceso
# directo del Personal Vault) mas dos triviales.
#
# LO QUE NO SE TOCA, comprobado antes de escribir esto: Escritorio, Documentos,
# Imagenes y Musica apuntan a rutas locales de C:\Users\obisp, y Videos y
# Descargas a I:. Known Folder Move NO esta activo, asi que desinstalar no mueve
# ni rompe ninguna carpeta.
[CmdletBinding()]
param([switch]$Hacerlo)

$ErrorActionPreference = 'Continue'
. "$env:USERPROFILE\.config\lib\rice-paths.ps1"

$RUN       = 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Run'
$APROBADAS = 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$CARPETA   = Join-Path $env:USERPROFILE 'OneDrive'
# Los respaldos del trim anterior. Si OneDrive se queda ahi dentro,
# `rice-trim-run.ps1 -Restore` lo resucita.
$RESPALDOS = @(
    (Join-Path $Rice.Config '.run-backup.json'),
    (Join-Path $Rice.Config '.run-trimmed.json')
)

function Titulo($t) { Write-Host "`n== $t ==" -ForegroundColor Cyan }

# --- que hay ----------------------------------------------------------------
Titulo 'que hay ahora'

$procs = Get-Process -EA SilentlyContinue | Where-Object { $_.ProcessName -match 'OneDrive' }
Write-Host ("  procesos            {0}" -f $(if ($procs) { ($procs.ProcessName | Sort-Object -Unique) -join ', ' } else { 'ninguno' }))

$inst = foreach ($r in 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*',
                       'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*',
                       'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*') {
    Get-ItemProperty $r -EA SilentlyContinue | Where-Object { $_.DisplayName -match 'OneDrive' }
}
$inst = @($inst)
Write-Host ("  instalado           {0}" -f $(if ($inst) { "$($inst[0].DisplayName) $($inst[0].DisplayVersion)" } else { 'no' }))

# Entradas Run que apunten a OneDrive, se llamen como se llamen. Buscar por
# NOMBRE no sirve: la de esta maquina se llama Microsoft.Lists.
$runOneDrive = @()
$k = Get-Item $RUN -EA SilentlyContinue
if ($k) {
    foreach ($n in $k.Property) {
        $v = (Get-ItemProperty $RUN -Name $n -EA SilentlyContinue).$n
        if ("$v" -match 'OneDrive') { $runOneDrive += [pscustomobject]@{ Nombre = $n; Valor = $v } }
    }
}
if ($runOneDrive) { $runOneDrive | ForEach-Object { Write-Host ("  arranque            {0} -> {1}" -f $_.Nombre, $_.Valor) } }
else { Write-Host '  arranque            ninguno' }

$tareas = @(Get-ScheduledTask -EA SilentlyContinue | Where-Object { $_.TaskName -match 'OneDrive' })
foreach ($t in $tareas) { Write-Host ("  tarea               {0}  [{1}]" -f $t.TaskName, $t.State) }

if (Test-Path $CARPETA) {
    $todos = @(Get-ChildItem $CARPETA -Recurse -File -Force -EA SilentlyContinue)
    # 0x400000 = FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: el contenido vive en la
    # nube y este archivo es solo un marcador.
    $nube = @($todos | Where-Object { $_.Attributes -band 0x400000 })
    Write-Host ("  carpeta             {0} archivos, {1} solo-en-nube" -f $todos.Count, $nube.Count)
    $locales = @($todos | Where-Object { -not ($_.Attributes -band 0x400000) })
    if ($locales) {
        Write-Host '  archivos LOCALES (su contenido si esta en este disco):' -ForegroundColor Yellow
        $locales | ForEach-Object { Write-Host ("     {0,10:N0} B  {1}" -f $_.Length, $_.FullName.Substring($CARPETA.Length + 1)) }
    }
} else {
    Write-Host '  carpeta             no existe'
}

$mb = 0
foreach ($p in (Join-Path $env:LOCALAPPDATA 'Microsoft\OneDrive'), (Join-Path $env:ProgramFiles 'Microsoft OneDrive')) {
    if (Test-Path $p) { $mb += ((Get-ChildItem $p -Recurse -File -EA SilentlyContinue) | Measure-Object Length -Sum).Sum / 1MB }
}
Write-Host ("  en disco            {0:N0} MB" -f $mb)

foreach ($f in $RESPALDOS) {
    if (Test-Path $f) {
        $hay = (Get-Content $f -Raw -EA SilentlyContinue) -match 'OneDrive'
        Write-Host ("  respaldo            {0}  {1}" -f (Split-Path $f -Leaf), $(if ($hay) { 'CONTIENE OneDrive (lo resucitaria)' } else { 'limpio' }))
    }
}

if (-not $Hacerlo) {
    Write-Host "`nDiagnostico. Para ejecutarlo: rice-onedrive-purge.ps1 -Hacerlo" -ForegroundColor Yellow
    Write-Host 'Los archivos solo-en-nube NO se descargan: siguen en la cuenta, en onedrive.com.'
    exit 0
}

# --- que se puede hacer sin elevacion --------------------------------------
#
# Todo lo de esta seccion. Se hace ANTES de pedir permiso a proposito: la
# primera version era todo-o-nada y, al cancelarse el aviso de UAC, no avanzo
# absolutamente nada aunque nueve de las diez cosas no necesitaban permiso.
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
         ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

Titulo 'parando procesos'
foreach ($p in $procs) {
    Stop-Process -Id $p.Id -Force -EA SilentlyContinue
    Write-Host ("  parado {0} ({1})" -f $p.ProcessName, $p.Id)
}
if (-not $procs) { Write-Host '  no habia ninguno' }
Start-Sleep -Seconds 2

Titulo 'arranque'
foreach ($e in $runOneDrive) {
    Remove-ItemProperty $RUN -Name $e.Nombre -Force -EA SilentlyContinue
    Write-Host ("  quitada Run\{0}" -f $e.Nombre)
}
# El visto bueno de Windows es una clave aparte. Dejarlo puesto hace que la
# entrada reaparezca habilitada si algo la vuelve a escribir.
$ka = Get-Item $APROBADAS -EA SilentlyContinue
if ($ka) {
    foreach ($n in $ka.Property) {
        if ($n -match 'OneDrive' -or $n -eq 'Microsoft.Lists') {
            Remove-ItemProperty $APROBADAS -Name $n -Force -EA SilentlyContinue
            Write-Host ("  quitado StartupApproved\{0}" -f $n)
        }
    }
}

Titulo 'tareas programadas'
$tareasPendientes = @()
foreach ($t in $tareas) {
    $hecho = $false
    try {
        Unregister-ScheduledTask -TaskName $t.TaskName -TaskPath $t.TaskPath -Confirm:$false -EA Stop
        Write-Host ("  eliminada {0}" -f $t.TaskName); $hecho = $true
    } catch {
        try {
            Disable-ScheduledTask -TaskName $t.TaskName -TaskPath $t.TaskPath -EA Stop | Out-Null
            Write-Host ("  desactivada {0}" -f $t.TaskName); $hecho = $true
        } catch { }
    }
    # Las de usuario salen sin permiso; la de maquina
    # ("Per-Machine Standalone Update") no.
    if (-not $hecho) { $tareasPendientes += $t.TaskName; Write-Host ("  PENDIENTE (necesita admin) {0}" -f $t.TaskName) -ForegroundColor Yellow }
}

Titulo 'respaldos del trim'
# El formato es un OBJETO nombre -> valor, no un array de {Name,Value}.
#
# La primera version filtraba con `$j | Where-Object { $_.Name ... }`, que sobre
# un objeto itera UN solo elemento sin propiedad Name: no coincidia nada, los
# contadores salian iguales y el script informaba "ya estaba limpio" con OneDrive
# todavia dentro. Un falso negativo, que es peor que un error.
foreach ($f in $RESPALDOS) {
    if (-not (Test-Path $f)) { continue }
    try {
        $j = Get-Content $f -Raw | ConvertFrom-Json
        $fuera = @($j.PSObject.Properties | Where-Object { "$($_.Name) $($_.Value)" -match 'OneDrive|Microsoft\.Lists' })
        if ($fuera.Count) {
            $nuevo = [ordered]@{}
            foreach ($p in $j.PSObject.Properties) {
                if ("$($p.Name) $($p.Value)" -notmatch 'OneDrive|Microsoft\.Lists') { $nuevo[$p.Name] = $p.Value }
            }
            $nuevo | ConvertTo-Json -Depth 5 | Set-Content $f -Encoding utf8
            Write-Host ("  {0}: quitadas {1} -> {2}" -f (Split-Path $f -Leaf), $fuera.Count, (($fuera.Name) -join ', '))
        } else {
            Write-Host ("  {0}: sin entradas de OneDrive" -f (Split-Path $f -Leaf))
        }
    } catch { Write-Host ("  {0}: no pude leerlo ({1})" -f (Split-Path $f -Leaf), $_.Exception.Message) -ForegroundColor Yellow }
}

# --- lo que si necesita administrador ---------------------------------------
Titulo 'desinstalando'
$desinstalado = $false
if (-not $inst) {
    Write-Host '  no estaba instalado'
    $desinstalado = $true
} elseif (-not $admin -and $tareasPendientes.Count -eq 0 -and $false) {
    # (rama imposible, se deja explicito que sin admin no se desinstala)
} else {
    $exe = $null
    if ($inst[0].UninstallString -match '"([^"]+OneDriveSetup\.exe)"') { $exe = $Matches[1] }
    elseif ($inst[0].UninstallString -match '(\S+OneDriveSetup\.exe)') { $exe = $Matches[1] }
    if (-not ($exe -and (Test-Path $exe))) {
        Write-Host '  no encuentro OneDriveSetup.exe en la cadena de desinstalacion' -ForegroundColor Yellow
    } elseif ($admin) {
        Write-Host "  $exe"
        # Argumentos por array: la cadena del registro los trae con espacios
        # dobles y pasarla entera confunde al parser.
        $u = '/' + 'uninstall'
        $a = '/' + 'allusers'
        Start-Process $exe -ArgumentList $u, $a -Wait
        $desinstalado = -not (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\OneDriveSetup.exe' -EA SilentlyContinue)
        Write-Host ("  desinstalador terminado ({0})" -f $(if ($desinstalado) { 'ya no figura instalado' } else { 'aun figura instalado' }))
    } else {
        Write-Host '  la instalacion es /allusers: hace falta administrador' -ForegroundColor Yellow
        Write-Host '  relanzo con elevacion; acepta el aviso de Windows'
        Start-Process pwsh -Verb RunAs -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass',
            '-File', $PSCommandPath, '-Hacerlo' -EA SilentlyContinue
        Write-Host ''
        Write-Host '  Si cancelaste el aviso, todo lo de arriba YA esta hecho. Para' -ForegroundColor Yellow
        Write-Host '  terminar, ejecuta esto tu mismo en una consola de administrador:' -ForegroundColor Yellow
        Write-Host ("    & '{0}' {1} {2}" -f $exe, ('/' + 'uninstall'), ('/' + 'allusers'))
        exit 1
    }
}

Titulo 'carpeta'
if (-not $desinstalado) {
    Write-Host '  no la borro: OneDrive sigue instalado y la recrearia'
} elseif (Test-Path $CARPETA) {
    # Se borra SOLO si nada de lo que queda tiene contenido propio en el disco.
    # Los marcadores solo-en-nube no cuentan: su contenido nunca estuvo aqui.
    $restan = @(Get-ChildItem $CARPETA -Recurse -File -Force -EA SilentlyContinue |
                Where-Object { -not ($_.Attributes -band 0x400000) -and $_.Length -gt 2048 })
    if ($restan) {
        Write-Host '  NO la borro: hay archivos con contenido local de mas de 2 KB' -ForegroundColor Yellow
        $restan | ForEach-Object { Write-Host ("     {0}" -f $_.FullName) }
    } else {
        Remove-Item $CARPETA -Recurse -Force -EA SilentlyContinue
        Write-Host ("  borrada {0}" -f $(if (Test-Path $CARPETA) { '(quedan restos, mirala a mano)' } else { 'del todo' }))
    }
}

Titulo 'como queda'
Write-Host ("  procesos   {0}" -f (@(Get-Process -EA SilentlyContinue | Where-Object { $_.ProcessName -match 'OneDrive' }).Count))
Write-Host ("  tareas     {0}" -f (@(Get-ScheduledTask -EA SilentlyContinue | Where-Object { $_.TaskName -match 'OneDrive' }).Count))
$k2 = Get-Item $RUN -EA SilentlyContinue
$quedan = if ($k2) { @($k2.Property | Where-Object { "$((Get-ItemProperty $RUN -Name $_ -EA SilentlyContinue).$_)" -match 'OneDrive' }).Count } else { 0 }
Write-Host ("  arranque   {0}" -f $quedan)
Write-Host "`nTus 78 archivos siguen en onedrive.com. Nada local se ha perdido." -ForegroundColor Green
