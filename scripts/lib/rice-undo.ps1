# Deshacer unificado para los scripts del rice.
#
# EL PROBLEMA QUE RESUELVE
#
# Habia SIETE scripts que tocaban el sistema y cada uno se invento su propio
# respaldo: .accent-backup.json, .run-trimmed.json, .retired-run.json, una
# carpeta con fecha en logs\, y dos que directamente no guardaban nada y
# "restauraban" borrando el valor. Eso ultimo es el fallo de fondo:
#
#   rice-notif-banners.ps1 -Restore  hacia  Remove-ItemProperty ShowBanner
#
# Si la app tenia ShowBanner=1 puesto a mano ANTES, el "restore" no lo devolvia:
# lo borraba. No distinguir "no existia" de "valia 0" es la forma mas comun de
# escribir un undo que miente.
#
# LA IDEA
#
# La funcion que ESCRIBE es la misma que APUNTA COMO DESHACER. No se puede
# olvidar, porque no hay dos pasos. Prestado de optimizerDuck (GPL, asi que aqui
# no hay una linea de su codigo: solo la idea y las claves del sistema, que son
# hechos y no son de nadie).
#
# USO
#
#   . "$env:USERPROFILE\.config\lib\rice-undo.ps1"
#
#   Start-RiceUndo 'accent'                          # abre un ambito
#   Set-RegValueTracked 'HKCU:\...\DWM' 'AccentColor' 0xff5c3a -Kind DWord
#   Remove-RegValueTracked 'HKCU:\...\Run' 'OneDrive'
#   Register-RiceUndo -Tipo servicio -Datos @{ Nombre='WSearch'; Inicio='Automatic' }
#   Save-RiceUndo                                    # lo persiste
#
#   Undo-Rice 'accent'      # lo deshace todo, en orden inverso
#   Get-RiceUndo            # que ambitos hay guardados y de cuando
#
# QUE NO HACE, dicho claro:
#
#   - No comprueba si TU cambiaste el valor despues. Al deshacer, pisa.
#   - No deshace desinstalaciones de Appx ni borrados de archivos. Para eso no
#     hay marcha atras y fingir lo contrario seria peor.

$ErrorActionPreference = 'Stop'

$script:RiceUndoDir     = Join-Path $env:USERPROFILE '.config\undo'
$script:RiceUndoVersion = 1

# Ambito en curso. Es el equivalente pobre del AsyncLocal de C#: una variable de
# script. Vale porque estos scripts son de un solo hilo.
$script:RiceUndoAmbito  = $null
$script:RiceUndoPasos   = $null

# ---------------------------------------------------------------- rutas ------

# Acepta 'HKCU:\Software\X', 'HKCU\Software\X' y 'HKEY_CURRENT_USER\Software\X'.
# Devuelve @{ Raiz = [RegistryKey]; Sub = 'Software\X'; Texto = 'HKCU:\Software\X' }
function ConvertTo-RiceRegPath {
    param([Parameter(Mandatory)][string]$Ruta)

    $r = $Ruta -replace '^Registry::', ''
    $r = $r -replace '^([A-Za-z_]+):\\', '$1\'

    $partes = $r -split '\\', 2
    $raizNombre = $partes[0].ToUpperInvariant()
    $sub = if ($partes.Count -gt 1) { $partes[1] } else { '' }

    $mapa = @{
        'HKCU' = @([Microsoft.Win32.Registry]::CurrentUser,  'HKCU')
        'HKEY_CURRENT_USER' = @([Microsoft.Win32.Registry]::CurrentUser, 'HKCU')
        'HKLM' = @([Microsoft.Win32.Registry]::LocalMachine, 'HKLM')
        'HKEY_LOCAL_MACHINE' = @([Microsoft.Win32.Registry]::LocalMachine, 'HKLM')
        'HKCR' = @([Microsoft.Win32.Registry]::ClassesRoot,  'HKCR')
        'HKEY_CLASSES_ROOT' = @([Microsoft.Win32.Registry]::ClassesRoot, 'HKCR')
        'HKU'  = @([Microsoft.Win32.Registry]::Users,        'HKU')
        'HKEY_USERS' = @([Microsoft.Win32.Registry]::Users,  'HKU')
    }
    if (-not $mapa.ContainsKey($raizNombre)) {
        throw "raiz de registro desconocida: '$raizNombre' (de '$Ruta')"
    }
    @{ Raiz = $mapa[$raizNombre][0]; Sub = $sub; Texto = "$($mapa[$raizNombre][1]):\$sub" }
}

# ---------------------------------------------------------------- ambito -----

function Start-RiceUndo {
    param(
        [Parameter(Mandatory)][string]$Ambito,
        # Por defecto se ACUMULA sobre lo que ya hubiera guardado de ese ambito.
        # Empezar de cero borraria el undo de una ejecucion anterior que quiza
        # nunca se deshizo.
        [switch]$Nuevo
    )
    $script:RiceUndoAmbito = $Ambito
    $script:RiceUndoPasos  = New-Object System.Collections.ArrayList

    if (-not $Nuevo) {
        $previos = Get-RiceUndoPasos $Ambito
        foreach ($p in $previos) { [void]$script:RiceUndoPasos.Add($p) }
    }
}

function Save-RiceUndo {
    if (-not $script:RiceUndoAmbito) { return }
    if ($script:RiceUndoPasos.Count -eq 0) {
        # Nada que deshacer: no dejes un fichero vacio que parezca estado.
        $f = Get-RiceUndoFile $script:RiceUndoAmbito
        if (Test-Path $f) { Remove-Item $f -Force -EA SilentlyContinue }
        $script:RiceUndoAmbito = $null
        return
    }
    Write-RiceUndoFile $script:RiceUndoAmbito @($script:RiceUndoPasos)
    $script:RiceUndoAmbito = $null
    $script:RiceUndoPasos  = $null
}

function Get-RiceUndoFile {
    param([Parameter(Mandatory)][string]$Ambito)
    # Nombre saneado: el ambito lo pone el script, no el usuario, pero un
    # '..\..' aqui escribiria donde no toca.
    $limpio = ($Ambito -replace '[^A-Za-z0-9_.-]', '_')
    Join-Path $script:RiceUndoDir "$limpio.json"
}

function Write-RiceUndoFile {
    param([Parameter(Mandatory)][string]$Ambito, [array]$Pasos)
    if (-not (Test-Path $script:RiceUndoDir)) {
        New-Item -ItemType Directory -Force -Path $script:RiceUndoDir | Out-Null
    }
    $f = Get-RiceUndoFile $Ambito
    $doc = [ordered]@{
        version  = $script:RiceUndoVersion
        ambito   = $Ambito
        aplicado = (Get-Date).ToString('o')
        pasos    = $Pasos
    }
    # Escribir-y-renombrar: si el proceso muere a media escritura, el fichero
    # viejo sigue intacto. Un undo corrupto es peor que un undo antiguo.
    #
    # Move-Item -Force y no [System.IO.File]::Move/Replace: el Move de tres
    # argumentos no existe en PowerShell 5.1 (es de .NET Core), y a Replace
    # PowerShell le convierte el $null del tercer parametro en cadena vacia y
    # revienta con "La ruta de acceso no tiene un formato valido". Move-Item
    # -Force acaba en MoveFileEx con REPLACE_EXISTING, que en el mismo volumen
    # es atomico igual. Estos scripts corren tanto con pwsh como con powershell.
    $tmp = "$f.tmp"
    $doc | ConvertTo-Json -Depth 8 -Compress | Set-Content $tmp -Encoding utf8
    Move-Item -LiteralPath $tmp -Destination $f -Force
}

function Get-RiceUndoPasos {
    param([Parameter(Mandatory)][string]$Ambito)
    $f = Get-RiceUndoFile $Ambito
    if (-not (Test-Path $f)) { return @() }
    try { $doc = Get-Content $f -Raw | ConvertFrom-Json } catch {
        # Ilegible: se aparta en vez de tratarse como vacio, que lo borraria.
        Move-Item $f "$f.bad" -Force -EA SilentlyContinue
        Write-Warning "undo de '$Ambito' ilegible; apartado en $f.bad"
        return @()
    }
    if ($doc.version -ne $script:RiceUndoVersion) {
        Write-Warning "undo de '$Ambito' es version $($doc.version), esperaba $script:RiceUndoVersion"
        return @()
    }
    @($doc.pasos)
}

function Register-RiceUndo {
    param(
        [Parameter(Mandatory)][ValidateSet('reg-valor','reg-claves','servicio','tarea','comando')]
        [string]$Tipo,
        [Parameter(Mandatory)][hashtable]$Datos
    )
    if (-not $script:RiceUndoAmbito) {
        throw "Register-RiceUndo sin ambito. Llama antes a Start-RiceUndo."
    }
    $paso = [ordered]@{ i = $script:RiceUndoPasos.Count; tipo = $Tipo }
    foreach ($k in $Datos.Keys) { $paso[$k] = $Datos[$k] }
    [void]$script:RiceUndoPasos.Add([pscustomobject]$paso)
}

# ------------------------------------------------------------- escritura -----

# Crea el camino de subclaves y devuelve las que ha creado ESTE proceso, de mas
# somera a mas profunda. Solo esas se borran al deshacer.
function New-RiceRegPath {
    param([Parameter(Mandatory)]$Raiz, [Parameter(Mandatory)][string]$Sub, [string]$Prefijo)
    $creadas = @()
    if (-not $Sub) { return @{ Clave = $Raiz.OpenSubKey('', $true); Creadas = $creadas } }

    $actual = $Raiz
    $acum = ''
    foreach ($parte in ($Sub -split '\\' | Where-Object { $_ })) {
        $acum = if ($acum) { "$acum\$parte" } else { $parte }
        $sig = $actual.OpenSubKey($parte, $true)
        if ($null -eq $sig) {
            $sig = $actual.CreateSubKey($parte, $true)
            if ($null -eq $sig) { throw "no pude crear $Prefijo\$acum" }
            $creadas += "$Prefijo\$acum"
        }
        if (-not $actual.Equals($Raiz)) { $actual.Dispose() }
        $actual = $sig
    }
    @{ Clave = $actual; Creadas = $creadas }
}

# Escribe un valor de registro Y apunta como devolverlo a como estaba.
#
# TRES DETALLES que casi ningun script de PowerShell hace y son justo los que
# separan un undo real de uno que miente:
#
#  1. DoNotExpandEnvironmentNames al LEER el respaldo. Sin eso, un REG_EXPAND_SZ
#     que valia '%SystemRoot%\...' se lee ya expandido a 'C:\Windows\...', y al
#     restaurarlo dejas una ruta fija donde habia una variable.
#  2. Distinguir "no existia" de "valia 0". Si no existia, deshacer es BORRAR el
#     valor, no escribir un 0 que nunca estuvo ahi.
#  3. Apuntar que subclaves ha creado uno. Al deshacer se borran, pero solo las
#     que sigan vacias y empezando por la mas profunda.
function Set-RegValueTracked {
    param(
        [Parameter(Mandatory)][string]$Ruta,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Nombre,
        [Parameter(Mandatory)]$Valor,
        [Microsoft.Win32.RegistryValueKind]$Kind = [Microsoft.Win32.RegistryValueKind]::Unknown
    )
    $p = ConvertTo-RiceRegPath $Ruta
    $prefijo = ($p.Texto -split '\\', 2)[0]        # 'HKCU:'
    $res = New-RiceRegPath $p.Raiz $p.Sub $prefijo
    $clave = $res.Clave
    $creadas = @($res.Creadas)

    try {
        $existia = $false
        $anterior = $null
        $kindAnterior = [Microsoft.Win32.RegistryValueKind]::Unknown

        $nombres = $clave.GetValueNames()
        if ($nombres -contains $Nombre) {
            $existia = $true
            $anterior = $clave.GetValue(
                $Nombre, $null,
                [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $kindAnterior = $clave.GetValueKind($Nombre)
        }

        if ($Kind -eq [Microsoft.Win32.RegistryValueKind]::Unknown) {
            $clave.SetValue($Nombre, $Valor)
        } else {
            $clave.SetValue($Nombre, $Valor, $Kind)
        }

        # ORDEN IMPORTANTE: primero las claves, luego el valor.
        #
        # Al deshacer se recorre al reves (LIFO), asi que el valor se borra
        # ANTES de intentar borrar la clave que lo contiene. Registrandolo al
        # reves, la clave se evaluaba con el valor todavia dentro, no estaba
        # vacia, y no se borraba nunca. Costo un test rojo verlo.
        if ($creadas.Count -gt 0) {
            Register-RiceUndo -Tipo reg-claves -Datos @{ creadas = $creadas }
        }
        Register-RiceUndo -Tipo reg-valor -Datos @{
            ruta    = $p.Texto
            nombre  = $Nombre
            existia = $existia
            valor   = (ConvertTo-RiceRegSerializable $anterior $kindAnterior)
            kind    = $kindAnterior.ToString()
        }
    } finally {
        if ($clave -and -not $clave.Equals($p.Raiz)) { $clave.Dispose() }
    }
}

# Borra un valor Y apunta como devolverlo. Si no existia, no apunta nada: no hay
# nada que deshacer y un paso vacio solo ensucia el log.
function Remove-RegValueTracked {
    param(
        [Parameter(Mandatory)][string]$Ruta,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Nombre
    )
    $p = ConvertTo-RiceRegPath $Ruta
    $clave = $p.Raiz.OpenSubKey($p.Sub, $true)
    if ($null -eq $clave) { return $false }
    try {
        if (($clave.GetValueNames()) -notcontains $Nombre) { return $false }
        $anterior = $clave.GetValue(
            $Nombre, $null,
            [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $kind = $clave.GetValueKind($Nombre)

        $clave.DeleteValue($Nombre, $false)

        Register-RiceUndo -Tipo reg-valor -Datos @{
            ruta    = $p.Texto
            nombre  = $Nombre
            existia = $true
            valor   = (ConvertTo-RiceRegSerializable $anterior $kind)
            kind    = $kind.ToString()
        }
        return $true
    } finally { $clave.Dispose() }
}

# JSON no tiene byte[] ni distingue int de long. Se serializa segun el tipo del
# registro y se reconstruye igual al deshacer, o Binary vuelve como array de
# numeros y MultiString como cadena suelta.
function ConvertTo-RiceRegSerializable {
    param($Valor, [Microsoft.Win32.RegistryValueKind]$Kind)
    if ($null -eq $Valor) { return $null }
    switch ($Kind) {
        ([Microsoft.Win32.RegistryValueKind]::Binary) {
            return [Convert]::ToBase64String([byte[]]$Valor)
        }
        ([Microsoft.Win32.RegistryValueKind]::MultiString) { return @([string[]]$Valor) }
        ([Microsoft.Win32.RegistryValueKind]::DWord)  { return [int]$Valor }
        ([Microsoft.Win32.RegistryValueKind]::QWord)  { return [long]$Valor }
        default { return [string]$Valor }
    }
}

function ConvertFrom-RiceRegSerializable {
    param($Valor, [string]$KindTexto)
    $kind = [Microsoft.Win32.RegistryValueKind]::Unknown
    if ($KindTexto) { $kind = [Microsoft.Win32.RegistryValueKind]$KindTexto }
    switch ($kind) {
        ([Microsoft.Win32.RegistryValueKind]::Binary) {
            return @{ V = [Convert]::FromBase64String([string]$Valor); K = $kind }
        }
        ([Microsoft.Win32.RegistryValueKind]::MultiString) {
            return @{ V = [string[]]@($Valor); K = $kind }
        }
        ([Microsoft.Win32.RegistryValueKind]::DWord) { return @{ V = [int]$Valor;  K = $kind } }
        ([Microsoft.Win32.RegistryValueKind]::QWord) { return @{ V = [long]$Valor; K = $kind } }
        default { return @{ V = $Valor; K = $kind } }
    }
}

# --------------------------------------------------------------- deshacer ----

function Undo-Rice {
    param(
        [Parameter(Mandatory)][string]$Ambito,
        [switch]$Simular
    )
    $pasos = Get-RiceUndoPasos $Ambito
    if ($pasos.Count -eq 0) { Write-Host "nada guardado para '$Ambito'."; return }

    # LIFO: se deshace en orden inverso al que se hizo. Al reves, un paso que
    # creo una clave se desharia antes que el valor que metio dentro.
    $orden = @($pasos | Sort-Object -Property i -Descending)
    $ok = 0; $fallos = @()

    foreach ($paso in $orden) {
        try {
            if ($Simular) {
                Write-Host ("  [simular] {0,-11} {1}" -f $paso.tipo, (Format-RiceUndoPaso $paso))
                $ok++
                continue
            }
            switch ($paso.tipo) {
                'reg-valor'  { Undo-RiceRegValor  $paso }
                'reg-claves' { Undo-RiceRegClaves $paso }
                'servicio'   { Undo-RiceServicio  $paso }
                'tarea'      { Undo-RiceTarea     $paso }
                'comando'    { Undo-RiceComando   $paso }
                default      { throw "tipo de paso desconocido: $($paso.tipo)" }
            }
            $ok++
        } catch {
            $fallos += [pscustomobject]@{ Paso = $paso; Error = $_.Exception.Message }
            Write-Warning "paso $($paso.i) ($($paso.tipo)) fallo: $($_.Exception.Message)"
        }
    }

    if ($Simular) { Write-Host "`n$ok paso(s) se ejecutarian. Nada tocado."; return }

    if ($fallos.Count -eq 0) {
        Remove-Item (Get-RiceUndoFile $Ambito) -Force -EA SilentlyContinue
        Write-Host "'$Ambito' deshecho: $ok paso(s). Registro borrado."
    } else {
        # NO se borra el registro si algo fallo.
        #
        # optimizerDuck tiene justo este bug (RevertManager.cs:138): borra los
        # datos de undo si al menos UN paso salio bien, asi que un fallo parcial
        # deja pasos irreversibles para siempre. Aqui se conserva entero: se
        # puede reintentar, y los que ya se deshicieron son idempotentes.
        $quedan = @($pasos | Where-Object { $f = $_; $fallos.Paso.i -contains $f.i })
        Write-Warning "'$Ambito': $ok bien, $($fallos.Count) fallidos. El registro NO se borra: reintenta con Undo-Rice '$Ambito'."
    }
}

function Undo-RiceRegValor {
    param($Paso)
    $p = ConvertTo-RiceRegPath $Paso.ruta
    $clave = $p.Raiz.OpenSubKey($p.Sub, $true)
    if ($null -eq $clave) {
        if (-not $Paso.existia) { return }   # no habia valor y no hay clave: ya esta
        $res = New-RiceRegPath $p.Raiz $p.Sub 'x'
        $clave = $res.Clave
    }
    try {
        if ($Paso.existia) {
            $d = ConvertFrom-RiceRegSerializable $Paso.valor $Paso.kind
            if ($d.K -eq [Microsoft.Win32.RegistryValueKind]::Unknown) {
                $clave.SetValue($Paso.nombre, $d.V)
            } else {
                $clave.SetValue($Paso.nombre, $d.V, $d.K)
            }
        } else {
            # No existia: deshacer es BORRARLO, no escribir un cero.
            if (($clave.GetValueNames()) -contains $Paso.nombre) {
                $clave.DeleteValue($Paso.nombre, $false)
            }
        }
    } finally { if ($clave) { $clave.Dispose() } }
}

# Borra solo las subclaves que creamos nosotros, de mas profunda a menos, y solo
# si siguen COMPLETAMENTE vacias. Si alguien metio algo dentro entretanto, se
# queda: no es nuestra para borrarla.
function Undo-RiceRegClaves {
    param($Paso)
    $ordenadas = @($Paso.creadas | Sort-Object -Property { ($_ -split '\\').Count } -Descending)
    foreach ($ruta in $ordenadas) {
        $p = ConvertTo-RiceRegPath $ruta
        $k = $p.Raiz.OpenSubKey($p.Sub, $false)
        if ($null -eq $k) { continue }
        $vacia = ($k.SubKeyCount -eq 0 -and $k.ValueCount -eq 0)
        $k.Dispose()
        if (-not $vacia) { continue }
        $padreSub = $p.Sub -replace '\\[^\\]+$', ''
        $hoja = ($p.Sub -split '\\')[-1]
        if ($padreSub -eq $p.Sub) { continue }   # es hija directa de la raiz
        $padre = $p.Raiz.OpenSubKey($padreSub, $true)
        if ($padre) { try { $padre.DeleteSubKey($hoja, $false) } finally { $padre.Dispose() } }
    }
}

function Undo-RiceServicio {
    param($Paso)
    Set-Service -Name $Paso.nombre -StartupType $Paso.inicio -ErrorAction Stop
}

function Undo-RiceTarea {
    param($Paso)
    if ($Paso.activada) {
        Enable-ScheduledTask -TaskName $Paso.nombre -TaskPath $Paso.ruta -EA Stop | Out-Null
    } else {
        Disable-ScheduledTask -TaskName $Paso.nombre -TaskPath $Paso.ruta -EA Stop | Out-Null
    }
}

# Ultimo recurso: un comando inverso guardado como texto. Menos fiable que un
# snapshot -- si el mundo cambio, el comando puede no aplicar -- asi que se usa
# solo cuando no hay estado que capturar.
function Undo-RiceComando {
    param($Paso)
    $sb = [scriptblock]::Create($Paso.comando)
    & $sb
}

function Format-RiceUndoPaso {
    param($Paso)
    switch ($Paso.tipo) {
        'reg-valor'  { if ($Paso.existia) { "$($Paso.ruta) :: $($Paso.nombre) = $($Paso.valor) [$($Paso.kind)]" } else { "$($Paso.ruta) :: $($Paso.nombre) -> BORRAR (no existia)" } }
        'reg-claves' { "borrar si vacias: $($Paso.creadas -join ', ')" }
        'servicio'   { "$($Paso.nombre) -> $($Paso.inicio)" }
        'tarea'      { "$($Paso.ruta)$($Paso.nombre) -> $(if ($Paso.activada) { 'activada' } else { 'desactivada' })" }
        'comando'    { $Paso.comando }
        default      { '?' }
    }
}

# ----------------------------------------------------------------- listar ----

function Get-RiceUndo {
    param([string]$Ambito)
    if (-not (Test-Path $script:RiceUndoDir)) { Write-Host 'no hay nada que deshacer.'; return }
    $fichs = Get-ChildItem $script:RiceUndoDir -Filter *.json -EA SilentlyContinue
    if ($Ambito) { $fichs = $fichs | Where-Object { $_.BaseName -eq $Ambito } }
    if (-not $fichs) { Write-Host 'no hay nada que deshacer.'; return }

    foreach ($f in $fichs | Sort-Object LastWriteTime -Descending) {
        try { $doc = Get-Content $f.FullName -Raw | ConvertFrom-Json } catch { continue }
        $cuando = try { [datetime]::Parse($doc.aplicado).ToString('yyyy-MM-dd HH:mm') } catch { '?' }
        Write-Host ("{0,-18} {1}  {2,3} paso(s)" -f $doc.ambito, $cuando, @($doc.pasos).Count)
        if ($Ambito) {
            foreach ($p in @($doc.pasos | Sort-Object i)) {
                Write-Host ("   {0,3}  {1,-11} {2}" -f $p.i, $p.tipo, (Format-RiceUndoPaso $p))
            }
        }
    }
}
