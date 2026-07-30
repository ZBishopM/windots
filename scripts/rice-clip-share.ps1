# Sube un clip: lo transcodifica, lo pone en catbox, y avisa a Discord SOLO si
# el clip es de League.
#
#   rice-clip-share.ps1 <ruta.mp4>            decide el destino por si mismo
#   rice-clip-share.ps1 <ruta.mp4> -Game      fuerza el aviso a Discord
#   rice-clip-share.ps1 <ruta.mp4> -NoDiscord sube y calla
#
# Lo llama shadowplay-wgc-save.ps1 en segundo plano, para que Alt+F10 siga siendo
# instantaneo: entre transcodificar y subir hay ~15 s y nadie quiere esperarlos
# con el juego en pausa.
#
# POR QUE SE TRANSCODIFICA, que es la parte que no es obvia: el grabador produce
# HEVC (H.265), y Discord es Chromium, que no decodifica H.265. Un clip subido
# tal cual le sale a tus amigos como una descarga en vez de reproducirse en el
# canal, y el enlace de catbox tampoco se previsualiza. Con NVENC cuesta ~6 s.
#
# Lo que se compra es que se REPRODUZCA, no que ocupe menos. El tamano depende
# del contenido y va en las dos direcciones: medidos en esta maquina, un clip
# bajo de 25,9 a 13,8 MB y otro SUBIO de 24,5 a 27,3. No es un fallo -- HEVC es
# ~40% mas eficiente a igual calidad, asi que reencodear al mismo cq sube el
# bitrate. Por eso hay un tope mas abajo: si el H.264 engorda mas de la mitad,
# se descarta.
#
# EL FOCO SE COMPRUEBA EN QUIEN LLAMA, no aqui. Cuando este script termina de
# subir han pasado ~15 s y ya has salido del juego, asi que preguntar por la
# ventana enfocada en este punto diria "no" siempre. El que guarda mira el foco
# en el instante de Alt+F10 y nos pasa -Game.
[CmdletBinding()]
param(
    [Parameter(Mandatory, Position = 0)][string]$Clip,
    [switch]$Game,
    [switch]$NoDiscord
)

$ErrorActionPreference = 'Continue'
. "$env:USERPROFILE\.config\lib\rice-paths.ps1"

$log = Join-Path $Rice.LogDir 'clip-share.log'
New-Item -ItemType Directory -Force $Rice.LogDir -EA SilentlyContinue | Out-Null
function L($m) { "$(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') $m" | Add-Content -Path $log -Encoding utf8 }

if (-not (Test-Path -LiteralPath $Clip)) { L "no existe: $Clip"; exit 1 }

# --- configuracion ---------------------------------------------------------
# Lo NO secreto va en rice.json; la URL del webhook NO, porque este repo es
# publico y con esa URL cualquiera puede publicar en tu canal. Vive en
# rice-secrets.json, que no esta en el mapa de sync.ps1 ni se versiona.
# upload arranca en $false a proposito. Si rice.json falta o no se puede leer,
# el fallo seguro es NO publicar. Al reves -- que un archivo ilegible provoque
# una subida a un host publico -- no tiene arreglo despues: las subidas anonimas
# de catbox NO se pueden borrar (su API de borrado exige userhash y devuelve 412
# sin el).
$cfg = @{ upload = $false; host = 'catbox'; litterbox_time = '72h'; transcode = $true; cq = 26 }
try {
    $j = Get-Content (Join-Path $Rice.Config 'rice.json') -Raw | ConvertFrom-Json
    if ($j.clips) { foreach ($p in $j.clips.PSObject.Properties) { $cfg[$p.Name] = $p.Value } }
} catch { L "rice.json ilegible, uso valores por defecto: $($_.Exception.Message)" }

if (-not $cfg.upload) { L 'clips.upload = false; no subo nada'; exit 0 }

$hook = $null
$userhash = $null
$secrets = Join-Path $Rice.Config 'rice-secrets.json'
if (Test-Path $secrets) {
    try {
        $sj = Get-Content $secrets -Raw | ConvertFrom-Json
        $hook = $sj.discord_webhook
        $userhash = $sj.catbox_userhash
    } catch { L 'rice-secrets.json ilegible' }
}

# --- transcodificar --------------------------------------------------------
$subir = $Clip
$tmp = $null
if ($cfg.transcode) {
    $tmp = Join-Path $env:TEMP ("share_" + [IO.Path]::GetFileNameWithoutExtension($Clip) + ".mp4")
    Set-RiceIsland 'replay' 'Preparando clip' 'transcodificando a H.264' '#8fa1b3'
    $sw = [Diagnostics.Stopwatch]::StartNew()
    # cq en vez de bitrate fijo: el contenido de un juego varia mucho y un
    # bitrate constante desperdicia en las pausas y se queda corto en las peleas.
    # +faststart mueve el indice al principio, que es lo que permite empezar a
    # reproducir sin descargar el archivo entero.
    & $Rice.Ffmpeg -y -hide_banner -loglevel error -i $Clip `
        -c:v h264_nvenc -preset p5 -rc vbr -cq $cfg.cq -b:v 0 -maxrate 12M -bufsize 24M `
        -c:a copy -movflags +faststart $tmp 2>&1 | Out-Null
    $sw.Stop()
    if ((Test-Path $tmp) -and (Get-Item $tmp).Length -gt 0) {
        $antes = (Get-Item $Clip).Length
        $desp  = (Get-Item $tmp).Length
        L ("transcodificado en {0:N1}s: {1:N1} MB -> {2:N1} MB" -f $sw.Elapsed.TotalSeconds, ($antes/1MB), ($desp/1MB))
        # H.264 puede salir MAS GRANDE que el HEVC de origen, y no es un fallo:
        # HEVC es ~40% mas eficiente a igual calidad, asi que reencodear a la
        # misma calidad percibida (cq) sube el bitrate. Medido aqui las dos
        # cosas: un clip bajo de 25,9 a 13,8 MB y otro subio de 24,5 a 27,3.
        # Se sube el H.264 igualmente cuando engorda poco -- lo que se compra es
        # que se REPRODUZCA, y un HEVC que nadie puede ver no sirve de nada --
        # pero si se pasa de la mitad mas, se descarta y se avisa.
        if ($desp -gt $antes * 1.5) {
            L 'el H.264 engorda mas de 50%: me quedo con el HEVC original'
            Remove-Item $tmp -Force -EA SilentlyContinue
            $tmp = $null
        } else {
            $subir = $tmp
        }
    } else {
        # Se sube el original antes que no subir nada, pero queda dicho: puede
        # que no se reproduzca en el canal.
        L 'la transcodificacion fallo; subo el HEVC original (puede no reproducirse)'
        $tmp = $null
    }
}

$mb = (Get-Item -LiteralPath $subir).Length / 1MB

# --- subir -----------------------------------------------------------------
$esLitter = $cfg.host -eq 'litterbox'
$api  = if ($esLitter) { 'https://litterbox.catbox.moe/resources/internals/api.php' } else { 'https://catbox.moe/user/api.php' }
$tope = if ($esLitter) { 1024 } else { 200 }
if ($mb -gt $tope) {
    L ("{0:N1} MB pasa del limite de {1} ({2} MB)" -f $mb, $cfg.host, $tope)
    Set-RiceIsland 'warn' 'Clip demasiado grande' ("{0:N0} MB, el limite es {1} MB" -f $mb, $tope) '#d08770'
    if ($tmp) { Remove-Item $tmp -Force -EA SilentlyContinue }
    exit 1
}

Set-RiceIsland 'replay' 'Subiendo clip' ("{0:N1} MB a {1}" -f $mb, $cfg.host) '#8fa1b3'
$form = @{ reqtype = 'fileupload'; fileToUpload = Get-Item -LiteralPath $subir }
if ($esLitter) { $form.time = $cfg.litterbox_time }
# Con userhash la subida queda asociada a la cuenta, y eso es lo que la hace
# BORRABLE: sin el, catbox rechaza deletefiles con 412 y el archivo se queda
# publico para siempre (o hasta 2 anos sin visitas). Litterbox no lo admite:
# alli todo es anonimo y caduca solo, que es su forma de resolver lo mismo.
if ($userhash -and -not $esLitter) { $form.userhash = $userhash }

$url = $null
try {
    # -Form hace el multipart/form-data que pide la API (el mismo que usa la
    # config de ShareX que publican). Timeout generoso: son decenas de MB.
    $url = (Invoke-RestMethod -Uri $api -Method Post -Form $form -TimeoutSec 300).Trim()
} catch { L "la subida fallo: $($_.Exception.Message)" }

if ($tmp) { Remove-Item $tmp -Force -EA SilentlyContinue }

if (-not ($url -match '^https?://')) {
    L "respuesta inesperada de $($cfg.host): $url"
    Set-RiceIsland 'warn' 'Subida fallida' 'mira clip-share.log' '#d08770'
    exit 1
}
L "subido: $url"
Set-Clipboard -Value $url   # aunque no vaya a Discord, queda a mano

# --- avisar a Discord, solo si es de League --------------------------------
$aDiscord = $Game -and -not $NoDiscord -and $hook
if (-not $Game) {
    L 'no era League: no aviso a Discord'
} elseif ($NoDiscord) {
    L '-NoDiscord: no aviso'
} elseif (-not $hook) {
    L 'era League pero no hay discord_webhook en rice-secrets.json'
}

if ($aDiscord) {
    try {
        # Solo el enlace, no el archivo. El webhook usa el nivel de BOOST DEL
        # SERVIDOR (10 MB sin boost, 50 en nivel 2, 100 en nivel 3), y con 26 MB
        # de media un clip no cabe en un servidor sin boost. Un enlace cabe
        # siempre, y Discord previsualiza el .mp4 de H.264 en el propio canal.
        $cuerpo = @{ content = $url } | ConvertTo-Json -Compress
        Invoke-RestMethod -Uri $hook -Method Post -ContentType 'application/json' -Body $cuerpo -TimeoutSec 30 | Out-Null
        L 'publicado en Discord'
        Set-RiceIsland 'check' 'Clip compartido' 'en Discord y en el portapapeles' '#a9b56a'
    } catch {
        L "Discord fallo: $($_.Exception.Message)"
        Set-RiceIsland 'warn' 'Clip subido, Discord no' 'enlace en el portapapeles' '#d08770'
    }
} else {
    Set-RiceIsland 'check' 'Clip subido' 'enlace en el portapapeles' '#a9b56a'
}
