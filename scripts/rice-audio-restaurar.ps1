<#
  Deja los dispositivos de audio como corresponde despues de hibernar.

      rice-audio-restaurar.ps1            aplica
      rice-audio-restaurar.ps1 -Simular   dice que haria, sin tocar nada

  Al despertar, Windows reenumera los endpoints y el predeterminado se mueve
  solo: la salida termina en el monitor por HDMI en vez del headset, y el
  micro puede quedar en otro. Se vio en vivo cambiando dos veces en una misma
  sesion, y explica los huecos de audio de escritorio del 12 y del 22-23 de
  agosto en las grabaciones.

  Criterio, que NO es "forzar siempre lo mismo":

    Microfono -> se fija a rice.json/record_mic. Aqui si hay un valor
    correcto y unico: es el que se quiere grabar siempre.

    Salida    -> solo se corrige si el predeterminado se fue FUERA de
    rice.json/outputs. Entre los de la lista (headset, monitor, airpods) la
    eleccion es tuya y cambiarla a tus espaldas seria peor que el problema;
    la captura loopback tiene que tomar lo que realmente esta sonando. Lo que
    se corrige es la deriva a algo que nunca elegiste.

  Tambien reafirma el nivel del micro, porque un endpoint que vuelve al 58%
  (o mudo) captura voz que la mezcla entierra, y eso ya paso.

  Se engancha con rice-audio-restaurar-tarea.ps1 (tarea programada con
  disparador de reanudacion). Correrlo a mano es inofensivo e idempotente.
#>
param([switch]$Simular)

$ErrorActionPreference = 'Stop'
$exe    = Join-Path $env:USERPROFILE 'dev\target\release\micswitch.exe'
$config = Join-Path $env:USERPROFILE '.config\rice.json'
$log    = Join-Path $env:USERPROFILE '.config\audio-restaurar.log'

if (-not (Test-Path $exe))    { throw "no encuentro micswitch.exe en $exe" }
if (-not (Test-Path $config)) { throw "no encuentro rice.json en $config" }

$cfg        = Get-Content $config -Raw | ConvertFrom-Json
$microQuery = $cfg.record_mic
$salidas    = @($cfg.outputs)
$nivel      = if ($null -ne $cfg.record_mic_level) { [int]$cfg.record_mic_level } else { 100 }

function Escribir($texto) {
    $linea = "{0}  {1}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $texto
    Write-Host $linea
    # Solo en la corrida real: un -Simular no debe ensuciar el historial que
    # se consulta para saber que paso al despertar.
    if (-not $Simular) { Add-Content -Path $log -Value $linea }
}

# El nombre del predeterminado actual. micswitch marca con "*" el activo, y
# SOLO se le pasan banderas que reconoce: cualquier argumento que no entienda
# cae al camino de "ciclar", que cambiaria el dispositivo sin pedirlo.
function Predeterminado([switch]$Salida) {
    # Las dos llamadas van escritas enteras a proposito, sin splatting: en
    # PowerShell un @('--list') de un solo elemento se degrada a String, y
    # `& $exe @var` sobre un String no pasa "--list" sino sus caracteres
    # sueltos. micswitch no los reconoce y cae a su camino por defecto, que es
    # CICLAR el dispositivo -- o sea, consultarlo lo cambiaria. Ya paso.
    $salida_ = if ($Salida) { & $exe --output --list } else { & $exe --list }
    $linea = $salida_ | Where-Object { $_.StartsWith('* ') } | Select-Object -First 1
    if ($linea) { $linea.Substring(2).Trim() } else { '' }
}

$cambios = 0

# --- Microfono: valor unico correcto, se fija siempre ---------------------
$micActual = Predeterminado
if ($microQuery -and $micActual -notmatch [regex]::Escape($microQuery)) {
    if ($Simular) {
        Escribir "[simulado] microfono: '$micActual' -> se fijaria a '$microQuery'"
    } else {
        $nuevo = (& $exe --set $microQuery | Select-Object -Last 1)
        Escribir "microfono: '$micActual' -> '$nuevo'"
    }
    $cambios++
} else {
    Escribir "microfono OK: '$micActual'"
}

# --- Nivel del micro: se reafirma siempre, es barato y ya fallo antes -----
# Se lee del listado en vez de fijarlo a ciegas, para no escribir cuando ya
# esta bien y para que el log diga si venia mudo.
$fila = & $exe --level | Where-Object { $_ -match [regex]::Escape($microQuery) } | Select-Object -First 1
$pct  = if ($fila -match '(\d+)%') { [int]$Matches[1] } else { -1 }
$mudo = $fila -match 'MUDO'
if ($mudo) { Escribir "AVISO: '$microQuery' esta MUDO -- micswitch no lo desmutea, hay que hacerlo a mano" }
if ($pct -ge 0 -and $pct -ne $nivel) {
    if ($Simular) {
        Escribir "[simulado] nivel de '$microQuery': $pct% -> $nivel%"
    } else {
        & $exe --set $microQuery --level $nivel | Out-Null
        Escribir "nivel de '$microQuery': $pct% -> $nivel%"
    }
    $cambios++
} elseif ($pct -ge 0) {
    Escribir "nivel OK: $pct%"
}

# --- Salida: a la de mayor prioridad que este disponible ------------------
# rice.json/outputs se lee como lista de PREFERENCIA, en orden. Se corrige a la
# primera que exista ahora mismo entre las activas.
#
# La regla mas suave -- "solo corregir si se fue fuera de la lista" -- no sirve
# para el caso real: cuando el headset se apaga, Windows manda el
# predeterminado al monitor por HDMI, que TAMBIEN esta en la lista, y al volver
# el headset se queda ahi. Es exactamente el sintoma reportado. Para cambiar la
# preferencia se reordena outputs en rice.json.
$salActual   = Predeterminado -Salida
$disponibles = & $exe --output --list | ForEach-Object { $_.Substring(2).Trim() }
$objetivo    = $null   # nombre completo del endpoint
$objetivoQry = $null   # el texto de rice.json que lo eligio, que es lo que --set espera
foreach ($s in $salidas) {
    $hallada = $disponibles | Where-Object { $_ -match [regex]::Escape($s) } | Select-Object -First 1
    if ($hallada) { $objetivo = $hallada; $objetivoQry = $s; break }
}
if (-not $objetivo) {
    Escribir "AVISO: ninguna de rice.json/outputs esta activa; se deja '$salActual' como este"
} elseif ($salActual -ne $objetivo) {
    if ($Simular) {
        Escribir "[simulado] salida: '$salActual' -> '$objetivo'"
    } else {
        $nuevo = (& $exe --output --set $objetivoQry | Select-Object -Last 1)
        Escribir "salida: '$salActual' -> '$nuevo'"
    }
    $cambios++
} else {
    Escribir "salida OK: '$salActual' (la de mayor prioridad disponible)"
}

Escribir "cambios: $cambios"

# El log se consulta despues de un despertar raro; sin recorte crece para
# siempre y deja de ser consultable.
if (-not $Simular -and (Test-Path $log)) {
    $lineas = @(Get-Content $log)
    if ($lineas.Count -gt 400) { Set-Content $log -Value ($lineas | Select-Object -Last 200) }
}
