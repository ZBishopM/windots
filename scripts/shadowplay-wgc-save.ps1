# Stitch the last ~30s of the WGC rolling buffer into a replay clip.
#
#   -Foreground <proceso>   quien tenia el foco al pulsar el atajo. Lo manda
#                           wezterm-hotkey.ahk, que es lo unico que corre en ese
#                           instante; ver el comentario del bind !F10.
[CmdletBinding()]
param([string]$Foreground = '')
# Lo PRIMERO, antes de cualquier trabajo. La version anterior lo sellaba despues
# del bucle de espera, asi que el "margen" del log comparaba el final del clip
# con el final de la espera y no con la pulsacion: parecia medir la latencia y
# no la medía.
$pulsado = Get-Date
# Video segments (segNN.mp4) are video-only; each has parallel raw-PCM audio in
# lockstep: segNN.pcm (system audio) and segNN.mic.pcm (microphone), s16le 48k
# stereo. We concat the video + each audio ring; if the mic ring exists we mix
# system + mic into one track, otherwise system audio only.
. "$env:USERPROFILE\.config\lib\rice-paths.ps1"
# (rice-ipc is no longer needed here: the toast used to be positioned on the
#  focused monitor via GlazeWM IPC, and the toast is gone.)

# Que aplicacion estaba delante al pulsar el atajo. Decide si el clip se anuncia
# en Discord.
#
# Se PREFIERE lo que nos pasa quien llama, porque leerlo aqui llega tarde:
# lanzar pwsh -- incluso con 'Hide' -- asigna una consola, y esa ventana ya ha
# movido el foco cuando este script arranca. Medido en el log: dos clips de
# League seguidos quedaron registrados como "no era League" con la partida
# delante. wezterm-hotkey.ahk lo lee con WinGetProcessName("A"), que si corre en
# el instante de la pulsacion, y lo manda en -Foreground.
#
# El P/Invoke se queda como respaldo para cuando el script se invoca a mano.
$fgName = $Foreground
if (-not $fgName) {
    Add-Type -Namespace RiceFg -Name W -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
'@ -EA SilentlyContinue
    try {
        $fgPid = 0
        [void][RiceFg.W]::GetWindowThreadProcessId([RiceFg.W]::GetForegroundWindow(), [ref]$fgPid)
        if ($fgPid) { $fgName = (Get-Process -Id $fgPid -EA SilentlyContinue).ProcessName }
    } catch { }
}

# Se compara por SUBCADENA, que es lo que hace que valga igual el nombre con
# extension que devuelve AHK ('League of Legends.exe') y el sin ella del
# P/Invoke. El proceso en partida es 'League of Legends'; el cliente es
# 'LeagueClientUx' y a proposito NO coincide -- un clip del lobby no es jugada.
$juegos = @('league of legends')
try {
    $cfgJson = Get-Content (Join-Path $Rice.Config 'rice.json') -Raw | ConvertFrom-Json
    if ($cfgJson.clips.discord_when_focused) { $juegos = @($cfgJson.clips.discord_when_focused) }
} catch { }
$esJuego = $false
foreach ($g in $juegos) { if ($fgName -and $fgName.ToLower().Contains($g.ToLower())) { $esJuego = $true } }

$buf = $Rice.WgcBuffer
$out = $Rice.Clips
$ff  = $Rice.Ffmpeg
New-Item -ItemType Directory -Force -Path $out | Out-Null

# Only one save at a time: Alt+F10 and the bar's save button are independent
# triggers, and two overlapping runs used to stomp each other's temp files.
$saveLock = New-Object System.Threading.Mutex($false, 'Global\shadowplay-save')
try { if (-not $saveLock.WaitOne(15000)) { exit 1 } }
catch [System.Threading.AbandonedMutexException] { }   # previous save died; we own it now

$segs = Get-ChildItem "$buf\seg*.mp4" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime
if ($segs.Count -lt 2) { $saveLock.ReleaseMutex(); exit 1 }

# ---------------------------------------------------------------------------
# PEDIR EL CORTE, en vez de esperar al siguiente.
#
# El problema original: los segmentos duran 5 s y solo sirve un segmento
# cerrado, asi que al pulsar habia dos salidas malas y ninguna buena. Tirar el
# segmento en curso perdia entre 1,7 y 4,7 s -- justo lo que uno quiere guardar
# cuando pulsa. Esperar a que cerrara solo costaba lo mismo en espera (0-5 s).
#
# Se arregla en el grabador: rotar de segmento ya era O(1) (un hilo pre-crea el
# codificador siguiente), lo unico que faltaba era poder pedirlo. Ahora se
# señaliza 'Global\rice-shadowplay-cut' y el corte ocurre en el fotograma
# siguiente; el grabador deja escrito en last-finished.txt QUE segmento acaba de
# cerrar, asi que aqui no hay que adivinar por fechas ni sondear ffprobe.
#
# El grabador ignora un corte si el segmento tiene menos de 1 s (solo guarda un
# codificador de repuesto), de ahi que la espera maxima util sean ~4 s y no 9.
# ---------------------------------------------------------------------------
$marca   = Join-Path $buf 'last-finished.txt'
function Get-Marca { if (Test-Path $marca) { (Get-Content $marca -Raw -EA SilentlyContinue).Trim() } else { '' } }
$antes   = Get-Marca
$marcado = ''
try {
    $ev = [System.Threading.EventWaitHandle]::OpenExisting('Global\rice-shadowplay-cut')
    [void]$ev.Set()
    $ev.Dispose()
    $tope = (Get-Date).AddSeconds(4)
    while ((Get-Date) -lt $tope) {
        $ahora = Get-Marca
        if ($ahora -and $ahora -ne $antes) { $marcado = $ahora; break }
        Start-Sleep -Milliseconds 20
    }
} catch { }   # grabador parado o sin el evento: se cae al camino de respaldo

# Un segmento esta listo cuando su video Y su audio estan cerrados. No basta con
# el .mp4: el audio va en archivos .pcm paralelos que el grabador libera aparte,
# y leerlos antes de tiempo revienta con "lo esta usando otro proceso" -- que es
# como se manifestaba, con el guardado fallando dos de cada tres veces.
#
# Solo se usa en el camino de respaldo: con corte bajo demanda, el segmento que
# nombra la marca esta cerrado por construccion.
function Test-SegmentoLibre($mp4) {
    $base = [IO.Path]::ChangeExtension($mp4, $null).TrimEnd('.')
    foreach ($f in @($mp4, "$base.pcm", "$base.mic.pcm")) {
        if (-not (Test-Path -LiteralPath $f)) { continue }   # el mic puede no existir
        try {
            # FileShare.ReadWrite, no FileShare.Read. La diferencia no es de
            # matiz: 'Read' significa "abro para leer Y NO PERMITO que nadie
            # escriba", asi que choca con el descriptor abierto del grabador y
            # falla aunque el archivo sea perfectamente legible. El grabador usa
            # File::create, que en Windows comparte lectura, escritura y borrado
            # -- comprobado abriendo un .pcm en pleno crecimiento: con 'Read'
            # falla, con 'ReadWrite' abre siempre.
            #
            # Con esto los .pcm dejan de imponer espera. El .mp4 si la necesita
            # de verdad, porque no tiene atomo moov hasta que finish() lo cierra.
            $s = [IO.File]::Open($f, 'Open', 'Read', 'ReadWrite')
            $s.Close()
        } catch { return $false }
    }
    return $true
}

$RING    = 12   # debe coincidir con RING en crates/shadowplay-wgc/src/main.rs
$OBJETIVO = 30  # segundos de clip
if ($marcado) {
    # Seleccion POR ANILLO, hacia atras desde el segmento que acaba de cerrarse.
    #
    # Esto sustituye a ordenar por fecha, que tenia una trampa: el grabador vacia
    # el hueco SIGUIENTE del anillo por adelantado, y ese archivo de 0 bytes
    # queda con mtime fresco, o sea entre los mas nuevos al ordenar. Se colaba en
    # la seleccion, ffmpeg lo saltaba y el clip salia de 20 s en vez de 30.
    # Yendo hacia atras desde el indice marcado nunca se pisa: esta por delante.
    #
    # Y se coge POR TIEMPO ACUMULADO, no seis fijos. Seis fijos eran 30 s cuando
    # todos los segmentos duraban 5 s; ahora el ultimo se corta a media vida y
    # los seis daban 22-26 s (medido). El mtime de un segmento cerrado es el
    # instante en que se cerro, asi que la distancia entre el marcado y otro es
    # exactamente el metraje que hay entre medias -- sin ffprobe.
    #
    # Tope RING-1: uno de los huecos es siempre el pre-vaciado del grabador.
    $n = [int]($marcado -replace '\D', '')
    $sel = @()
    $ref = $null
    for ($k = 0; $k -lt ($RING - 1); $k++) {
        $i = ((($n - $k) % $RING) + $RING) % $RING
        $f = Get-Item (Join-Path $buf ('seg{0:00}.mp4' -f $i)) -EA SilentlyContinue
        if (-not $f -or $f.Length -lt 100KB) { break }   # hueco vaciado o arranque en frio
        $atras = if ($null -eq $ref) { $ref = $f.LastWriteTime; 0 }
                 else { ($ref - $f.LastWriteTime).TotalSeconds }
        # Una vuelta entera del anillo son 60 s; mas viejo que eso es de la
        # generacion anterior, no parte de este clip.
        if ($atras -gt 65) { break }
        # El corte va ANTES de añadir, y esa es toda la aritmetica: el mtime de un
        # segmento cerrado es cuando se cerro, asi que $atras de este es
        # exactamente el metraje que suman todos los ya elegidos. Comprobarlo
        # despues de añadir metia siempre un segmento de mas y el clip salia de
        # 32,7-38,5 s (medido) en vez de 30.
        if ($atras -ge $OBJETIVO) { break }
        $sel = , $f + $sel
    }
    $last = $sel
}
else {
    # Respaldo: grabador parado, o version vieja sin el evento. Comportamiento de
    # antes -- filtrar por tamano (fuera los huecos vaciados) y por estar libre
    # (fuera el que aun escribe).
    $segs = Get-ChildItem "$buf\seg*.mp4" -ErrorAction SilentlyContinue |
            Where-Object { $_.Length -gt 100KB -and (Test-SegmentoLibre $_.FullName) } |
            Sort-Object LastWriteTime
    $last = @($segs | Select-Object -Last 6)
}
if ($last.Count -lt 1) { $saveLock.ReleaseMutex(); exit 1 }

# Dejar por escrito hasta donde llega el clip y cuanto costo. 'espera' es el dato
# que de verdad interesa: de la pulsacion a tener el segmento en disco.
$finClip = $last[-1].LastWriteTime
("{0} seleccion: {1} segmentos [{2}] | corte={3} | espera {4:N2}s | fin del clip {5:HH:mm:ss.fff} | margen {6:N1}s" -f `
    (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $last.Count, (($last | ForEach-Object { $_.BaseName }) -join ','),
    $(if ($marcado) { $marcado } else { 'respaldo' }), ((Get-Date) - $pulsado).TotalSeconds,
    $finClip, ($finClip - $pulsado).TotalSeconds) |
    Add-Content -Path (Join-Path $Rice.LogDir 'clip-share.log') -Encoding utf8

# Per-run temp names under TEMP (not fixed names in the buffer dir, which the
# recorder is also writing), cleaned up in the finally below.
$tmp      = [System.IO.Path]::GetRandomFileName()
$listFile = Join-Path $env:TEMP "$tmp.concat.txt"
$sysOut   = Join-Path $env:TEMP "$tmp.sys.pcm"
$micOut   = Join-Path $env:TEMP "$tmp.mic.pcm"

try {

($last | ForEach-Object { "file '" + ($_.FullName -replace "'", "''") + "'" }) |
    Set-Content -Path $listFile -Encoding ascii

# Concat the matching per-segment PCM (same time order): system audio and mic.
$haveSys = $true
$haveMic = $true
$fsS = [System.IO.File]::Create($sysOut)
$fsM = [System.IO.File]::Create($micOut)
try {
    # Copiar con FileShare.ReadWrite, NO con ReadAllBytes.
    #
    # Aqui estaba la ultima espera obligatoria. ReadAllBytes abre con
    # FileShare.Read, que no significa "solo leo": significa "abro para leer Y NO
    # PERMITO que nadie escriba". El hilo de audio del grabador rota de archivo
    # en su siguiente lectura del tubo, ~21 ms despues del corte, asi que el .pcm
    # del segmento recien cerrado puede seguir abierto ese instante -- y
    # ReadAllBytes fallaba con "lo esta usando otro proceso". El grabador abre con
    # File::create, que comparte lectura, escritura y borrado; con ReadWrite abre
    # siempre, incluso en pleno crecimiento (comprobado en caliente).
    function Copy-Pcm($src, $dst) {
        $s = [IO.File]::Open($src, 'Open', 'Read', 'ReadWrite')
        try { $s.CopyTo($dst) } finally { $s.Close() }
    }
    foreach ($v in $last) {
        $ps = Join-Path $buf ($v.BaseName + '.pcm')
        $pm = Join-Path $buf ($v.BaseName + '.mic.pcm')
        if (Test-Path $ps) { Copy-Pcm $ps $fsS } else { $haveSys = $false }
        if (Test-Path $pm) { Copy-Pcm $pm $fsM } else { $haveMic = $false }
    }
} finally { $fsS.Close(); $fsM.Close() }   # a mid-write segment must not leak handles
$haveSys = $haveSys -and (Get-Item $sysOut).Length -gt 0
$haveMic = $haveMic -and (Get-Item $micOut).Length -gt 0

$ts = Get-Date -Format 'yyyyMMdd_HHmmss'
$dest = Join-Path $out "replay_$ts.mp4"
$pcm = @('-f', 's16le', '-ar', '48000', '-ac', '2')  # raw-PCM input flags
if ($haveSys -and $haveMic) {
    # Mix system audio + mic into one track. normalize=0 keeps both at full level
    # (edit the amix weights here to rebalance game vs voice).
    & $ff -hide_banner -loglevel error -f concat -safe 0 -i $listFile `
        @pcm -i $sysOut @pcm -i $micOut `
        -filter_complex '[1:a][2:a]amix=inputs=2:duration=longest:normalize=0[a]' `
        -map 0:v:0 -map '[a]' -c:v copy -c:a aac -b:a 160k -shortest -y $dest 2>$null
}
elseif ($haveSys) {
    & $ff -hide_banner -loglevel error -f concat -safe 0 -i $listFile `
        @pcm -i $sysOut -map 0:v:0 -map 1:a:0 -c:v copy -c:a aac -b:a 160k -shortest -y $dest 2>$null
}
else {
    & $ff -hide_banner -loglevel error -f concat -safe 0 -i $listFile -c copy -y $dest 2>$null
}
# Feedback goes to the dynamic island only. The standalone toast used to fire as
# well, because the bar was hidden under a fullscreen game -- but the game runs
# Borderless, so the bar (and the island with it) stays visible during play and
# the toast was just the same message twice.
#
# Write-then-rename: the bar polls this file, so a truncate-then-write could be
# observed half-written.
if (Test-Path $dest) {
    Set-RiceIsland 'replay' 'Replay guardado' (Split-Path $dest -Leaf) '#a9b56a'
    Write-Output $dest

    # La subida va aparte y desligada: transcodificar y subir son ~15 s y Alt+F10
    # tiene que devolver el control ya.
    #
    # CreateNoWindow, NO `Start-Process -WindowStyle Hidden`. No son lo mismo y la
    # diferencia se vio en el juego: -WindowStyle Hidden pide SW_HIDE en el
    # STARTUPINFO, pero la consola SE ASIGNA IGUAL -- y asignar una ventana nueva
    # encima de un juego en pantalla completa exclusiva fuerza un cambio de modo
    # de video, que lo minimiza. Con League en fullscreen eso pasaba en CADA
    # Alt+F10. CreateNoWindow no crea consola en absoluto, y es lo que ya usa
    # todo el arbol para esto (rice_common::win::CREATE_NO_WINDOW, y su comentario
    # dice literalmente "so spawning a helper never flashes a console").
    # Anotar que se detecto: la vez pasada hubo que deducirlo del log de
    # subidas, y ahi solo constaba el resultado ("no era League"), no el dato.
    ("{0} guardado {1} | foco='{2}' | juego={3}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), (Split-Path $dest -Leaf), $fgName, $esJuego) |
        Add-Content -Path (Join-Path $Rice.LogDir 'clip-share.log') -Encoding utf8

    $share = Join-Path $Rice.Config 'rice-clip-share.ps1'
    if (Test-Path $share) {
        $psi = New-Object System.Diagnostics.ProcessStartInfo
        $psi.FileName = 'pwsh'
        $psi.CreateNoWindow = $true
        $psi.UseShellExecute = $false   # obligatorio: con $true, CreateNoWindow se ignora
        foreach ($a in @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $share, $dest)) {
            [void]$psi.ArgumentList.Add($a)
        }
        if ($esJuego) { [void]$psi.ArgumentList.Add('-Game') }
        [void][System.Diagnostics.Process]::Start($psi)
    }
}
else {
    # ffmpeg failed (its stderr is discarded above). Say so, so Alt+F10 never
    # just silently does nothing.
    Set-RiceIsland 'warn' 'Replay falló' 'ffmpeg no produjo el clip' '#d08770'
}

}
finally {
    Remove-Item $listFile, $sysOut, $micOut -Force -ErrorAction SilentlyContinue
    try { $saveLock.ReleaseMutex() } catch {}
    $saveLock.Dispose()
}
