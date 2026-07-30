# Desinstalar aplicaciones desde la terminal, sin ir a Configuracion.
#
#   rice-uninstall.ps1            lista todo
#   rice-uninstall.ps1 discord    lista solo lo que coincida
#   rice-uninstall.ps1 -Size      ordena por tamano declarado, no por nombre
#
# De donde sale la lista, y por que de ahi: el registro. `winget list` tiene que
# parsearse de una tabla de ancho fijo que cambia con el idioma y con los nombres
# largos, y `Get-Package` se salta la mitad. Las claves Uninstall del registro
# las escribe todo instalador que se respete, estan en los dos hives y en las
# dos vistas (64 y 32 bits), y traen ya el comando exacto de desinstalacion.
#
# Nada se desinstala sin que lo confirmes escribiendo el numero y luego "s".
#
# TRES COSAS QUE ESTA VERSION NO HACE, porque la anterior si y mentia:
#
# 1. No dice "terminado" sin comprobarlo. Antes imprimia eso siempre, despues de
#    un Start-Process cuyo error tragaba el $ErrorActionPreference de arriba. Con
#    Resident Evil 4 eso fue exactamente lo que paso: su desinstalador vivia en
#    G:\re4\..., y G: ya no existe como volumen. No se ejecuto nada y la salida
#    dijo que todo bien. Ahora se relee la clave del registro y se informa de lo
#    que de verdad quedo.
#
# 2. No se cree el tamano. `EstimatedSize` lo escribe el instalador y puede ser
#    pura ficcion: HuionTablet declara 19.581.463 KB (19 GB) y su carpeta mide
#    60 MB medidos, 318 veces menos. Se sigue mostrando porque medir cada carpeta
#    al listar seria recorrer cientos de miles de archivos, pero se marca como
#    declarado, y del elegido se mide el tamano real antes de confirmar.
#
# 3. No trata igual lo instalado y lo huerfano. Guilty Gear ya no estaba en disco
#    -- H: quedo con 51,3 de 52 GB libres -- pero su clave del registro seguia
#    ahi, asi que reaparecia en la lista como si se pudiera desinstalar. Eso no
#    necesita un desinstalador, necesita borrar la clave.
[CmdletBinding()]
param([string]$Filtro, [switch]$Size)

# Ojo: NO se pone $ErrorActionPreference = 'SilentlyContinue' de forma global.
# Eso es lo que oculto el fallo de arriba. Cada lectura que puede no existir
# lleva su propio -EA, y lo que importa va con -ErrorAction Stop.

# El UninstallString viene tal cual lo dejo el instalador: unas veces con el
# ejecutable entrecomillado y argumentos detras, otras SIN comillas -- e Inno
# Setup, que es la mitad de esta lista, lo escribe sin ellas.
#
# Partir por el primer espacio cuando no hay comillas es el error obvio y estaba
# aqui: HuionTablet declara
#
#     C:\Program Files\HuionTablet\Uninstall.exe
#
# y el corte por el primer espacio da el ejecutable 'C:\Program' con el resto
# como argumento. Ni existe ni se puede lanzar. En la version anterior eso caia
# en el Start-Process silenciado y se anunciaba como "terminado"; aqui, ademas,
# hacia que la entrada se marcara como huerfana teniendo la carpeta intacta.
#
# Sin comillas hay que probar: la cadena entera primero, y si no es un archivo,
# el prefijo mas corto que si lo sea. Es la unica forma de saber donde acaba la
# ruta y empiezan los argumentos.
function Split-Uninstall([string]$cmd) {
    $cmd = $cmd.Trim()
    if ($cmd.StartsWith('"')) {
        $end = $cmd.IndexOf('"', 1)
        if ($end -lt 0) { return @{ Exe = $cmd.Trim('"'); Arg = '' } }
        return @{ Exe = $cmd.Substring(1, $end - 1); Arg = $cmd.Substring($end + 1).Trim() }
    }
    if (Test-Path -LiteralPath $cmd -PathType Leaf -EA SilentlyContinue) {
        return @{ Exe = $cmd; Arg = '' }
    }
    $idx = -1
    while (($idx = $cmd.IndexOf(' ', $idx + 1)) -ge 0) {
        $cand = $cmd.Substring(0, $idx)
        if (Test-Path -LiteralPath $cand -PathType Leaf -EA SilentlyContinue) {
            return @{ Exe = $cand; Arg = $cmd.Substring($idx + 1).Trim() }
        }
    }
    # Nada de lo probado existe. Se devuelve el corte por el primer espacio, que
    # es lo unico que queda, y quien llama lo detectara como huerfana.
    $sp = $cmd.IndexOf(' ')
    if ($sp -lt 0) { return @{ Exe = $cmd; Arg = '' } }
    @{ Exe = $cmd.Substring(0, $sp); Arg = $cmd.Substring($sp + 1).Trim() }
}

# --- scoop -----------------------------------------------------------------
# Aparte porque scoop no toca el registro: sus aplicaciones son directorios.
$apps = @()
$scoopDir = "$env:USERPROFILE\scoop\apps"
if (Test-Path $scoopDir) {
    foreach ($d in Get-ChildItem $scoopDir -Directory -EA SilentlyContinue) {
        if ($d.Name -eq 'scoop') { continue }
        $apps += [pscustomobject]@{
            Nombre = $d.Name; Editor = 'scoop'; MB = 0; Origen = 'scoop'
            Comando = "scoop uninstall $($d.Name)"; Ruta = $d.FullName
            Clave = $null; Huerfana = $false
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
    foreach ($e in Get-ItemProperty $k -EA SilentlyContinue) {
        if (-not $e.DisplayName) { continue }
        # Las actualizaciones y los componentes del sistema no son aplicaciones
        # que nadie quiera desinstalar desde aqui.
        if ($e.SystemComponent -eq 1) { continue }
        if ($e.ReleaseType -in 'Security Update', 'Update Rollup', 'Hotfix') { continue }
        if (-not $e.UninstallString) { continue }

        # Huerfana = la entrada existe pero lo que describe ya no. Dos senales, y
        # se piden las dos porque cada una sola da falsos positivos: msiexec
        # siempre existe (asi que su ruta no dice nada) y muchos instaladores no
        # rellenan InstallLocation (asi que su ausencia tampoco dice nada).
        $p = Split-Uninstall $e.UninstallString
        $esMsi = $p.Exe -match 'msiexec'
        $faltaExe = -not $esMsi -and -not (Test-Path $p.Exe -EA SilentlyContinue)
        $faltaDir = $e.InstallLocation -and -not (Test-Path $e.InstallLocation -EA SilentlyContinue)

        $apps += [pscustomobject]@{
            Nombre = $e.DisplayName
            Editor = if ($e.Publisher) { $e.Publisher } else { '-' }
            MB     = if ($e.EstimatedSize) { [int]($e.EstimatedSize / 1024) } else { 0 }
            Origen = if ($k -like 'HKCU*') { 'usuario' } else { 'equipo' }
            Comando  = $e.UninstallString
            Ruta     = $e.InstallLocation
            Clave    = $e.PSPath
            Huerfana = ($faltaExe -or $faltaDir)
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
    # 'MB?' con interrogacion a proposito: es el tamano que DECLARA el instalador.
    $tam = if ($_.MB -gt 0) { '{0,6:N0} MB?' -f $_.MB } else { '           ' }
    $marca = if ($_.Huerfana) { ' [huerfana]' } else { '' }
    Write-Host ("{0,3}. {1,-46} {2} {3,-8} {4}{5}" -f $i, `
        $(if ($_.Nombre.Length -gt 46) { $_.Nombre.Substring(0, 43) + '...' } else { $_.Nombre }), `
        $tam, $_.Origen, $_.Editor, $marca)
}
Write-Host ''
Write-Host '  MB? = tamano DECLARADO por el instalador, no medido. Puede ser falso.' -ForegroundColor DarkGray
if ($apps | Where-Object Huerfana) {
    Write-Host '  [huerfana] = la entrada sigue en el registro pero sus archivos ya no estan.' -ForegroundColor DarkGray
}
Write-Host ''
$sel = Read-Host 'numero a desinstalar (Enter para salir)'
if (-not $sel) { return }
$n = 0
if (-not [int]::TryParse($sel, [ref]$n) -or $n -lt 1 -or $n -gt $apps.Count) {
    Write-Host 'numero fuera de rango.' -ForegroundColor Yellow; return
}
$app = $apps[$n - 1]

Write-Host ''
Write-Host ("  aplicacion: {0}" -f $app.Nombre) -ForegroundColor Cyan
Write-Host ("  editor:     {0}" -f $app.Editor)

# El tamano real, medido, y solo del elegido: recorrer una carpeta es barato una
# vez y carisimo doscientas.
if ($app.Ruta -and (Test-Path $app.Ruta -EA SilentlyContinue)) {
    Write-Host '  midiendo la carpeta...' -NoNewline
    $real = (Get-ChildItem $app.Ruta -Recurse -File -EA SilentlyContinue | Measure-Object Length -Sum).Sum / 1MB
    Write-Host ("`r  en disco:   {0:N0} MB reales en {1}" -f $real, $app.Ruta)
    if ($app.MB -gt 0 -and $real -gt 0) {
        $veces = $app.MB / $real
        if ($veces -gt 2 -or $veces -lt 0.5) {
            Write-Host ("  (el instalador declaraba {0:N0} MB: {1:N0} veces la realidad)" -f $app.MB, $veces) -ForegroundColor Yellow
        }
    }
}

if ($app.Huerfana) {
    Write-Host ''
    Write-Host '  ESTA ENTRADA ESTA HUERFANA.' -ForegroundColor Yellow
    Write-Host ("  no existe: {0}" -f $(if ($app.Ruta -and -not (Test-Path $app.Ruta -EA SilentlyContinue)) { $app.Ruta } else { (Split-Uninstall $app.Comando).Exe }))
    Write-Host '  Los archivos ya no estan, asi que no hay nada que desinstalar: lo unico'
    Write-Host '  que queda es la clave del registro, que es la que la mantiene en la lista.'
    Write-Host ''
    if ((Read-Host 'borrar solo la clave del registro? (s/N)') -notmatch '^[sSyY]') { Write-Host 'cancelado.'; return }
    try {
        Remove-Item $app.Clave -Recurse -Force -ErrorAction Stop
        Write-Host 'clave borrada.' -ForegroundColor Green
    } catch {
        Write-Host ("no se pudo: {0}" -f $_.Exception.Message) -ForegroundColor Red
        if ($app.Origen -eq 'equipo') { Write-Host 'esta en HKLM: hace falta una consola como administrador.' }
    }
    return
}

Write-Host ("  se ejecuta: {0}" -f $app.Comando) -ForegroundColor Yellow
if ($app.Origen -eq 'equipo') { Write-Host '  (instalada para todo el equipo: pedira permisos de administrador)' }
Write-Host ''
if ((Read-Host 'seguro? (s/N)') -notmatch '^[sSyY]') { Write-Host 'cancelado.'; return }

if ($app.Origen -eq 'scoop') {
    & scoop uninstall $app.Nombre
    return
}

# Espacio libre antes, para poder decir cuanto se libero de verdad.
$unidad = if ($app.Ruta) { (Split-Path $app.Ruta -Qualifier) } else { $null }
$libreAntes = if ($unidad) {
    (Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$unidad'" -EA SilentlyContinue).FreeSpace
} else { $null }

$p = Split-Uninstall $app.Comando
Write-Host 'lanzando el desinstalador...'
try {
    if ($p.Arg) { Start-Process -FilePath $p.Exe -ArgumentList $p.Arg -ErrorAction Stop }
    else        { Start-Process -FilePath $p.Exe -ErrorAction Stop }
} catch {
    Write-Host ("NO ARRANCO: {0}" -f $_.Exception.Message) -ForegroundColor Red
    Write-Host 'nada se ha desinstalado.' -ForegroundColor Red
    return
}

# No se usa -Wait. Los desinstaladores de Inno Setup (unins000.exe) se copian a
# un temporal, lanzan la copia y el proceso original termina en el acto, asi que
# -Wait volvia enseguida y daba la desinstalacion por hecha cuando acababa de
# empezar. Lo que de verdad indica el final es que desaparezca la clave.
Write-Host 'esperando a que termine (Ctrl+C para dejar de esperar; el desinstalador sigue)...'
$fin = (Get-Date).AddMinutes(20)
while ((Get-Date) -lt $fin) {
    Start-Sleep -Seconds 2
    if (-not (Test-Path $app.Clave -EA SilentlyContinue)) { break }
}

Write-Host ''
if (Test-Path $app.Clave -EA SilentlyContinue) {
    Write-Host 'SIGUE REGISTRADA.' -ForegroundColor Yellow
    Write-Host '  o el desinstalador se cancelo, o fallo, o dejo la clave atras.'
    if ($app.Ruta) {
        $queda = Test-Path $app.Ruta -EA SilentlyContinue
        Write-Host ("  la carpeta {0}: {1}" -f $app.Ruta, $(if ($queda) { 'sigue ahi' } else { 'ya no esta -- entonces la clave quedo huerfana; vuelve a ejecutar esto y borrala' }))
    }
} else {
    Write-Host 'DESINSTALADA (su clave ya no esta en el registro).' -ForegroundColor Green
}

if ($libreAntes) {
    $libreAhora = (Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$unidad'" -EA SilentlyContinue).FreeSpace
    $lib = ($libreAhora - $libreAntes) / 1GB
    Write-Host ("  {0} libero {1:N1} GB   ({2:N1} GB libres ahora)" -f $unidad, $lib, ($libreAhora / 1GB))
}
