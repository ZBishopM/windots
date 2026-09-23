# Levanta el modelo local (Qwen3.6-35B-A3B) con llama-server.
#
#   rice-llm.ps1              arranca el servidor
#   rice-llm.ps1 -Tune        barre el reparto GPU/CPU y dice cual va mas rapido
#   rice-llm.ps1 -Stop        lo para
#   rice-llm.ps1 -Status      dice si esta vivo y cuanta VRAM y RAM usa
#   rice-llm.ps1 -Chat        abre la interfaz de chat en el navegador
#
# Por que este modelo y no otro, para esta maquina en concreto: es un MoE de 35B
# totales pero solo 3B ACTIVOS por token. El cuello de botella aqui no es la
# VRAM (12 GB en la 4070 SUPER) sino el ancho de banda de la RAM -- DDR4-3200 en
# doble canal, unos 45 GB/s reales. Un modelo denso de 27B tendria que leer 27B
# de parametros por token desde ahi e iria a paso de tortuga. Este lee ~1 GB.
#
# El reparto: las capas de atencion y los expertos compartidos van a la GPU, y
# los 256 expertos enrutados a la RAM. Eso es lo que hace `--n-cpu-moe`.
#
# DOS TRAMPAS QUE COSTARON MEDIDAS, y por eso estan escritas aqui:
#
# 1. llama-server abre CUATRO ranuras por defecto, cada una con el contexto
#    ENTERO. Con -c 32768 eso son 131072 tokens de cache KV, cuatro veces lo
#    necesario. Con eso la VRAM se quedo en 127 MiB libres, CUDA empezo a tirar
#    por PCIe y la generacion cayo a 2 tok/s. De ahi `--parallel 1`.
#
# 2. El optimo que da llama-bench NO es el optimo del servidor. El banco no
#    reserva el contexto completo ni compite con el navegador por la VRAM, asi
#    que dijo que lo mejor era --n-cpu-moe 18. Con el servidor real, 18 no cabe.
#
# 3. LA GRANDE: --no-mmap. Sin ella, los tensores que van a la CPU se quedan
#    MAPEADOS desde el archivo de 16,85 GB en disco en vez de cargarse en RAM.
#    Cada token que toca un experto es un fallo de pagina contra el disco. El
#    propio llama.cpp lo avisa al cargar:
#
#      tensor overrides to CPU are used with mmap enabled
#      - consider using --no-mmap for better performance
#
#    Medido en esta maquina, con el navegador abierto comiendose la cache de
#    archivos: 1,2 tok/s con mmap, 23,4 tok/s sin ella. Diecinueve veces. Se
#    paga en el arranque -- 112 s leyendo el modelo entero de disco en vez de
#    50 -- y esa es toda la contrapartida.
#
#    Por eso tambien el contexto baja de 32k a 16k y --n-cpu-moe sube a 28: con
#    Firefox abierto la GPU tiene menos sitio del que tenia en las pruebas, y
#    quedarse sin VRAM devuelve al mismo agujero por otra via (CUDA se desborda
#    a memoria compartida por PCIe).
#
# Y una nota de uso: Qwen3.6 es un modelo de RAZONAMIENTO. La respuesta llega en
# `reasoning_content` mientras piensa y en `content` al final. Con pocos tokens
# de limite se gasta el presupuesto pensando y `content` vuelve VACIO. Dale
# margen (600+) o te parecera que no responde.
[CmdletBinding()]
param(
    [switch]$Tune,
    [switch]$Stop,
    [switch]$Status,
    [switch]$Chat,
    [int]$CpuMoe = 0,
    [int]$Ctx = 16384,
    [int]$Port = 8080
)

# F: y no I:. El HDD lee a 193 MB/s y el NVMe a 2.572 (medido): el modelo de
# 16,85 GB tardaba ~110 s en cargar y ahora tarda ~15. La copia de I:\ai era un
# duplicado exacto y solo servia para tener el modelo en el disco lento.
$root   = 'F:\ai'

# EL FORK DE PrismML, no nuestro llama.cpp, y es a proposito.
#
# El preset [bonsai] usa pesos ternarios PQ2_0 con una transformada
# Walsh-Hadamard que NO esta en upstream. Nuestro b11056 los rechaza, y su
# propia ficha avisa de que si los confunde con Q2_0 "produce basura" sin decir
# nada -- fallo silencioso, como el de GGML_CUDA_FA_ALL_QUANTS.
#
# El fork es b10709, o sea 347 compilaciones POR DETRAS de nuestro b11056. Se
# comprobo antes de cambiar que tiene lo que este script necesita:
# --models-preset, --models-max, --no-models-autoload, --cache-ram y
# --load-mode. Y sirve los GGUF normales igual, asi que los presets del 35B
# siguen funcionando.
#
# PARA VOLVER ATRAS: cambiar estas dos rutas a "$root\llama.cpp\..." y usar
# cualquier preset que no sea [bonsai].
$server = "$root\llama.cpp-prism\llama-server.exe"
$bench  = "$root\llama.cpp-prism\llama-bench.exe"
$model  = "$root\models\Qwen3.6-35B-A3B-UD-Q3_K_XL.gguf"
$mmproj = "$root\models\mmproj-F16.gguf"
$tuned   = "$root\n-cpu-moe.txt"
$presets = "$root\presets.ini"

function VramFree {
    $o = & "$env:SystemRoot\System32\nvidia-smi.exe" --query-gpu=memory.free --format=csv,noheader,nounits
    [int]($o -replace '\D', '')
}
# POR PUERTO, no por nombre de proceso.
#
# Desde que `ojo` arranca su propio llama-server en el 8099, "hay un proceso
# llama-server" dejo de significar "el mio esta arrancado". Con el nombre a
# secas, este script decia "ya estaba arrancado" y abria el chat en el 8080,
# donde no escucha nadie.
function Alive {
    try { $null = Invoke-RestMethod "http://127.0.0.1:$Port/v1/models" -TimeoutSec 2; $true }
    catch { $false }
}

# Los procesos que son MIOS: los que escuchan en mi puerto. Hace falta para no
# matar el de `ojo` al parar este -- `Get-Process llama-server | Stop-Process`
# se los llevaba a los dos.
function MisProcesos {
    Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" -EA SilentlyContinue |
        Where-Object { $_.CommandLine -match "--port\s+$Port\b" }
}

# El modelo de `ojo` y el de aqui NO caben a la vez: 11,4 + 9,3 GB sobre 12,28.
# Quedarse sin VRAM hace que CUDA se desborde por PCIe y los dos se arrastren
# (ya paso: 2 tok/s con 127 MiB libres). `ojo.ps1` ya se niega a arrancar si
# este esta puesto; esto es la mitad simetrica.
function PararOjo {
    $p = @(Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" -EA SilentlyContinue |
           Where-Object { $_.CommandLine -match '--port\s+8099\b' })
    if (-not $p) { return }
    foreach ($x in $p) { Stop-Process -Id $x.ProcessId -Force -EA SilentlyContinue }
    Write-Host "   parado el modelo de ojo ($($p.Count)): no caben los dos en la tarjeta"
}

# Avisa por la isla de la barra. Estos comandos se lanzan desde Win+Space, sin
# terminal: una ventana de consola que aparece y desaparece no dice nada, y es
# justo lo que paso la primera vez que se uso el comando de arrancar.
function Notify([string]$title, [string]$body, [string]$accent = '#e0a35c') {
    $j = @{ icon = ''; title = $title; body = $body; accent = $accent } | ConvertTo-Json -Compress
    Set-Content -Path "$env:USERPROFILE\.config\island.json" -Value $j -Encoding UTF8
}

if ($Stop) {
    $mios = @(MisProcesos)
    if (-not $mios) { Notify 'Modelo local' 'no estaba corriendo'; return }
    foreach ($p in $mios) { Stop-Process -Id $p.ProcessId -Force -EA SilentlyContinue }
    Notify 'Modelo local' 'parado, RAM liberada'
    Write-Host 'parado.'
    return
}

# Abre la interfaz de chat. Es lo que llama-server sirve en su raiz.
if ($Chat) {
    if (-not (Alive)) { Notify 'Modelo local' 'no esta arrancado' '#c86464'; return }
    Start-Process "http://127.0.0.1:$Port"
    return
}

if ($Status) {
    if (-not (Alive)) { Write-Host 'no esta corriendo.'; return }
    $p = Get-Process llama-server
    Write-Host ('llama-server pid {0}   RAM {1:N0} MB   VRAM libre {2:N0} MB' -f `
        $p.Id, ($p.WorkingSet64 / 1MB), (VramFree))
    try {
        $h = Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 3
        Write-Host ('  /health: {0}' -f ($h | ConvertTo-Json -Compress))
    } catch { Write-Host '  el puerto todavia no responde (sigue cargando)' }
    return
}

if (-not (Test-Path $model)) { Write-Host "falta el modelo: $model" -ForegroundColor Yellow; return }

# --- barrido -------------------------------------------------------------
# Cuantas capas de expertos mandar a la CPU. Cuantas menos, mas trabajo hace la
# GPU y mas rapido va -- hasta que no cabe y CUDA se queda sin memoria. El punto
# optimo depende de que mas tengas abierto, asi que se mide en vez de suponerse.
if ($Tune) {
    Write-Host 'barriendo --n-cpu-moe (cada prueba tarda ~1 min)...'
    Write-Host ('VRAM libre ahora: {0:N0} MB' -f (VramFree))
    $best = $null; $bestTps = 0
    foreach ($n in 34, 28, 26, 24, 22, 20) {
        Write-Host ("`n--- n-cpu-moe = $n ---")
        # --no-mmap tambien aqui, o el banco mide otra cosa que el servidor.
        $out = & $bench -m $model -ngl 99 --n-cpu-moe $n --no-mmap -t 6 -p 256 -n 64 -r 2 2>&1
        $line = $out | Select-String 'tg\d+|tg ' | Select-Object -Last 1
        $tps = 0.0
        if ($out -join "`n" -match '\|\s*tg\d+\s*\|\s*([\d.]+)') { $tps = [double]$Matches[1] }
        if ($out -match 'out of memory|CUDA error') { Write-Host '   no cabe en la VRAM'; continue }
        Write-Host ("   {0:N1} tok/s" -f $tps)
        if ($tps -gt $bestTps) { $bestTps = $tps; $best = $n }
    }
    if ($best) {
        Set-Content $tuned $best
        Write-Host ("`nmejor: --n-cpu-moe {0} a {1:N1} tok/s   (guardado en {2})" -f $best, $bestTps, $tuned) -ForegroundColor Green
    } else { Write-Host 'ninguna combinacion funciono.' -ForegroundColor Yellow }
    return
}

# --- arrancar ------------------------------------------------------------
if (Alive) {
    Notify 'Modelo local' 'ya estaba arrancado; abriendo el chat'
    Start-Process "http://127.0.0.1:$Port"
    return
}
Notify 'Modelo local' 'router arrancando...'
PararOjo

# MODO ROUTER, no un modelo suelto.
#
# Arrancar con `-m modelo.gguf` deja el contexto y el reparto fijados en la
# linea de comandos, y el boton "load model" de la web da error porque no hay
# nada que cargar. En modo router se le pasa un INI de presets: cada seccion
# aparece en el selector de la web y se puede cambiar sin tocar la terminal, que
# era justo lo que hacia falta para buscar el punto dulce.
#
# --models-max 1 porque no hay ni VRAM ni RAM para dos a la vez.
# --no-models-autoload para que arranque en segundos y solo cargue el que se
# elija; cargar son ~2 minutos por lo de --no-mmap.
$args = @(
    '--models-preset', $presets,
    '--models-max', '1',
    '--no-models-autoload',
    '--host', '127.0.0.1',
    '--port', $Port
)
Start-Process -FilePath $server -ArgumentList $args -WindowStyle Hidden
for ($i = 0; $i -lt 60; $i++) {
    Start-Sleep -Seconds 1
    try {
        Invoke-RestMethod "http://127.0.0.1:$Port/v1/models" -TimeoutSec 2 | Out-Null
        Notify 'Modelo local' 'router listo: elige un preset en la web' '#8fbf6f'
        Start-Process "http://127.0.0.1:$Port"
        Write-Host ("router listo en {0}s -> http://127.0.0.1:{1}" -f $i, $Port) -ForegroundColor Green
        Write-Host 'Elige un preset en el selector de la web. Cargar tarda ~2 min.'
        return
    } catch { }
}
Notify 'Modelo local' 'no respondio' '#c86464'
Write-Host 'el router no respondio en 60s.' -ForegroundColor Yellow
