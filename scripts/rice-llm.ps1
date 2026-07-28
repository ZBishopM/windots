# Levanta el modelo local (Qwen3.6-35B-A3B) con llama-server.
#
#   rice-llm.ps1              arranca el servidor
#   rice-llm.ps1 -Tune        barre el reparto GPU/CPU y dice cual va mas rapido
#   rice-llm.ps1 -Stop        lo para
#   rice-llm.ps1 -Status      dice si esta vivo y cuanta VRAM y RAM usa
#
# Por que este modelo y no otro, para esta maquina en concreto: es un MoE de 35B
# totales pero solo 3B ACTIVOS por token. El cuello de botella aqui no es la
# VRAM (12 GB en la 4070 SUPER) sino el ancho de banda de la RAM -- DDR4-3200 en
# doble canal, unos 45 GB/s reales. Un modelo denso de 27B tendria que leer 27B
# de parametros por token desde ahi e iria a paso de tortuga. Este lee ~1 GB.
#
# El reparto: las capas de atencion y los expertos compartidos van a la GPU, y
# los 256 expertos enrutados a la RAM. Eso es lo que hace `--n-cpu-moe`.
[CmdletBinding()]
param(
    [switch]$Tune,
    [switch]$Stop,
    [switch]$Status,
    [int]$CpuMoe = 0,
    [int]$Ctx = 32768,
    [int]$Port = 8080
)

$root   = 'I:\ai'
$server = "$root\llama.cpp\llama-server.exe"
$bench  = "$root\llama.cpp\llama-bench.exe"
$model  = "$root\models\Qwen3.6-35B-A3B-UD-Q3_K_XL.gguf"
$mmproj = "$root\models\mmproj-F16.gguf"
$tuned  = "$root\n-cpu-moe.txt"

function VramFree {
    $o = & "$env:SystemRoot\System32\nvidia-smi.exe" --query-gpu=memory.free --format=csv,noheader,nounits
    [int]($o -replace '\D', '')
}
function Alive { [bool](Get-Process llama-server -EA SilentlyContinue) }

if ($Stop) {
    Get-Process llama-server -EA SilentlyContinue | Stop-Process -Force
    Write-Host 'parado.'
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
    foreach ($n in 40, 34, 28, 24, 20, 16) {
        Write-Host ("`n--- n-cpu-moe = $n ---")
        $out = & $bench -m $model -ngl 99 --n-cpu-moe $n -t 6 -p 256 -n 64 -r 2 2>&1
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
if (Alive) { Write-Host 'ya esta corriendo. -Stop para pararlo.'; return }
if ($CpuMoe -eq 0) {
    $CpuMoe = if (Test-Path $tuned) { [int](Get-Content $tuned) } else { 28 }
}

$args = @(
    '-m', $model,
    '--mmproj', $mmproj,      # entrada de imagenes: util para un asistente que mire la pantalla
    '-ngl', '99',             # todas las capas a la GPU...
    '--n-cpu-moe', $CpuMoe,   # ...menos los expertos de las primeras N, que van a RAM
    '-c', $Ctx,
    '-fa', 'on',              # flash attention: menos VRAM de KV y mas rapido
    '-t', '6',                # 6 nucleos fisicos; poner 12 con HT suele ir PEOR
    '--host', '127.0.0.1',    # solo local, nada expuesto a la red
    '--port', $Port,
    '--no-webui'
)
Write-Host ("arrancando con --n-cpu-moe {0}, contexto {1}..." -f $CpuMoe, $Ctx)
Start-Process -FilePath $server -ArgumentList $args -WindowStyle Hidden
for ($i = 0; $i -lt 120; $i++) {
    Start-Sleep -Seconds 1
    try {
        Invoke-RestMethod "http://127.0.0.1:$Port/health" -TimeoutSec 2 | Out-Null
        Write-Host ("listo en {0}s -> http://127.0.0.1:{1}" -f $i, $Port) -ForegroundColor Green
        return
    } catch { }
}
Write-Host 'no respondio en 120s; mira si el proceso sigue vivo con -Status.' -ForegroundColor Yellow
