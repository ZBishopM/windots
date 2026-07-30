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
#
#   rice-uninstall.ps1 -Limpiar   borra las claves huerfanas, con copia previa
[CmdletBinding()]
param([string]$Filtro, [switch]$Size, [switch]$Limpiar)

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
            Clave = $null; Huerfana = $false; DesinstRoto = $false
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

        # Huerfana = el PROGRAMA ya no esta, no que su desinstalador este roto.
        # La distincion importa y costo un falso positivo de nueve entradas: los
        # paquetes portables de WinGet guardan en UninstallString la ruta con la
        # VERSION dentro, asi que al actualizarse queda obsoleta aunque el
        # programa siga perfectamente instalado. Marcarlos como huerfanos y
        # ofrecer borrar su clave habria quitado la unica via de desinstalarlos.
        #
        # Asi que manda InstallLocation cuando existe, y la ruta del
        # desinstalador solo se usa cuando no hay nada mejor:
        #
        #   InstallLocation presente y existe  -> instalado (aunque falte el .exe)
        #   InstallLocation presente y NO existe -> huerfana, sin dudas
        #   sin InstallLocation                -> se juzga por el desinstalador
        #
        # msiexec se excluye del juicio porque siempre existe: su presencia no
        # dice nada sobre si el producto sigue ahi.
        $p = Split-Uninstall $e.UninstallString
        $esMsi = $p.Exe -match 'msiexec'
        $faltaExe = -not $esMsi -and -not (Test-Path -LiteralPath $p.Exe -EA SilentlyContinue)

        # InstallLocation se normaliza antes de comprobarlo: hay instaladores que
        # lo escriben ENTRECOMILLADO, y entonces Test-Path falla por las comillas
        # y no por ausencia. En esta maquina son tres -- art-chat-rs,
        # live-subs-tauri (los dos Tauri) y el instalador de Visual Studio -- y
        # los tres existen; sin esto los tres salian como huerfanos y se habria
        # ofrecido borrar la clave de programas instalados.
        $dir = if ($e.InstallLocation) { $e.InstallLocation.Trim().Trim('"') } else { '' }

        if ($dir) {
            $hayDir = Test-Path -LiteralPath $dir -EA SilentlyContinue
            $huerfana = -not $hayDir
            $desinstRoto = $hayDir -and $faltaExe
        } else {
            $huerfana = $faltaExe
            $desinstRoto = $false
        }

        $apps += [pscustomobject]@{
            Nombre = $e.DisplayName
            Editor = if ($e.Publisher) { $e.Publisher } else { '-' }
            MB     = if ($e.EstimatedSize) { [int]($e.EstimatedSize / 1024) } else { 0 }
            Origen = if ($k -like 'HKCU*') { 'usuario' } else { 'equipo' }
            Comando  = $e.UninstallString
            Ruta     = $dir
            Clave    = $e.PSPath
            Huerfana = $huerfana
            DesinstRoto = $desinstRoto
        }
    }
}

$apps = $apps | Sort-Object Nombre -Unique

# --- limpieza de huerfanas -------------------------------------------------
# Por que se acumulan, que es la pregunta de fondo: NADA en Windows valida estas
# claves. En la practica la rama Uninstall es de solo-anadir -- cada instalador
# escribe la suya y unicamente un desinstalador que se porte bien la retira. Si
# borras la carpeta a mano, si desconectas el disco donde vivia, o si el
# desinstalador falla a medias, la clave se queda para siempre. En esta maquina
# eso dio: dos unidades que ya no existen (E: y G:), una biblioteca de Steam
# entera que se movio de C:\Program Files (x86)\Steam a D:\Steam, juegos
# borrados a mano, y PWAs de Brave cuyo perfil se limpio.
#
# Se borran SOLO las huerfanas -- programa ausente -- y nunca las de
# desinstalador roto, que estan instaladas.
if ($Limpiar) {
    $basura = $apps | Where-Object { $_.Huerfana -and $_.Clave }
    if (-not $basura) { Write-Host 'no hay claves huerfanas.' -ForegroundColor Green; return }

    Write-Host ("{0} claves huerfanas:" -f $basura.Count) -ForegroundColor Cyan
    $basura | Sort-Object Nombre | ForEach-Object {
        Write-Host ("  {0,-46} {1}" -f $_.Nombre.Substring(0, [Math]::Min(46, $_.Nombre.Length)), $_.Ruta)
    }
    $hklm = $basura | Where-Object { $_.Clave -notlike '*HKEY_CURRENT_USER*' }
    $esAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
               ).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    Write-Host ''
    Write-Host ("  {0} en HKLM (hacen falta permisos de administrador) - los tienes: {1}" -f $hklm.Count, $esAdmin)
    Write-Host '  NO se toca nada marcado [desinst.roto]: eso esta instalado.' -ForegroundColor DarkGray
    Write-Host ''
    if ((Read-Host 'borrar esas claves? (s/N)') -notmatch '^[sSyY]') { Write-Host 'cancelado.'; return }

    # Copia primero, y de las tres ramas enteras, no clave por clave: si algo se
    # va de las manos, un doble clic en el .reg lo devuelve todo.
    $bak = "$env:USERPROFILE\.config\logs\uninstall-backup-$(Get-Date -Format yyyyMMdd-HHmmss)"
    New-Item (Split-Path $bak) -ItemType Directory -Force -EA SilentlyContinue | Out-Null
    $ramas = @{
        'HKLM64' = 'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall'
        'HKLM32' = 'HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall'
        'HKCU'   = 'HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall'
    }
    foreach ($r in $ramas.GetEnumerator()) {
        $f = "$bak-$($r.Key).reg"
        & reg.exe export $r.Value $f /y *>$null
        Write-Host ("  copia: {0}  ({1:N0} KB)" -f $f, ((Get-Item $f -EA SilentlyContinue).Length / 1KB))
    }

    $ok = 0; $fail = 0
    foreach ($b in $basura) {
        try { Remove-Item $b.Clave -Recurse -Force -ErrorAction Stop; $ok++ }
        catch { $fail++; Write-Host ("  no se pudo: {0} -- {1}" -f $b.Nombre, $_.Exception.Message) -ForegroundColor Red }
    }
    Write-Host ''
    Write-Host ("borradas {0}, fallaron {1}." -f $ok, $fail) -ForegroundColor $(if ($fail) { 'Yellow' } else { 'Green' })
    if ($fail) { Write-Host 'las que fallaron viven en HKLM: relanza esto en una consola como administrador.' }
    Write-Host "para deshacer: doble clic en $bak-*.reg"
    return
}

if ($Filtro) { $apps = $apps | Where-Object { $_.Nombre -like "*$Filtro*" -or $_.Editor -like "*$Filtro*" } }
$apps = if ($Size) { $apps | Sort-Object MB -Descending } else { $apps | Sort-Object Nombre }

if (-not $apps) { Write-Host "nada coincide con '$Filtro'." -ForegroundColor Yellow; return }

$i = 0
$apps | ForEach-Object {
    $i++
    # 'MB?' con interrogacion a proposito: es el tamano que DECLARA el instalador.
    $tam = if ($_.MB -gt 0) { '{0,6:N0} MB?' -f $_.MB } else { '           ' }
    $marca = if ($_.Huerfana) { ' [huerfana]' } elseif ($_.DesinstRoto) { ' [desinst.roto]' } else { '' }
    Write-Host ("{0,3}. {1,-46} {2} {3,-8} {4}{5}" -f $i, `
        $(if ($_.Nombre.Length -gt 46) { $_.Nombre.Substring(0, 43) + '...' } else { $_.Nombre }), `
        $tam, $_.Origen, $_.Editor, $marca)
}
Write-Host ''
Write-Host '  MB? = tamano DECLARADO por el instalador, no medido. Puede ser falso.' -ForegroundColor DarkGray
if ($apps | Where-Object Huerfana) {
    Write-Host '  [huerfana] = la entrada sigue en el registro pero sus archivos ya no estan.' -ForegroundColor DarkGray
}
if ($apps | Where-Object DesinstRoto) {
    Write-Host '  [desinst.roto] = esta instalado, pero la ruta de su desinstalador ya no existe.' -ForegroundColor DarkGray
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

if ($app.DesinstRoto) {
    Write-Host ''
    Write-Host '  SU DESINSTALADOR NO ESTA DONDE DICE EL REGISTRO.' -ForegroundColor Yellow
    Write-Host ("  no existe: {0}" -f (Split-Uninstall $app.Comando).Exe)
    Write-Host '  El programa SI esta instalado (su carpeta existe), asi que la clave no'
    Write-Host '  sobra: lo que esta obsoleto es la ruta. Tipico de los portables de WinGet,'
    Write-Host '  que la guardan con la version dentro. Desinstalalo por su gestor'
    Write-Host '  (winget uninstall <id>, o scoop uninstall) en vez de desde aqui.'
    Write-Host ''
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
