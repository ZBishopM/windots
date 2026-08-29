<#
  Escucha por separado lo que el grabador capturo, para saber QUE fuente falla.

      shadowplay-verificar.ps1            corta ahora y separa las pistas
      shadowplay-verificar.ps1 -SinCorte  usa lo que ya haya en el bufer

  En el clip final las dos fuentes van mezcladas en una sola pista, asi que
  cuando algo no se oye es imposible saber si el problema fue la captura o la
  mezcla. Aqui salen dos WAV aparte -- sistema y microfono -- mas el nivel
  medido de cada uno, que es lo que distingue "no se capturo" de "se capturo
  muy bajo y la mezcla lo enterro".

  Deja los WAV en ShadowPlay\verificar\ y NO toca los clips.
#>
param([switch]$SinCorte)

$ErrorActionPreference = 'Stop'
$buf    = Join-Path $env:USERPROFILE 'ShadowPlay\wgc-buffer'
$salida = Join-Path $env:USERPROFILE 'ShadowPlay\verificar'
$ff     = 'ffmpeg'

if (-not $SinCorte) {
    # Mismo evento con nombre que usa Alt+F10: vuelca el anillo a disco.
    Add-Type -Namespace SPV -Name Ev -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet=System.Runtime.InteropServices.CharSet.Unicode, SetLastError=true)]
public static extern System.IntPtr OpenEventW(uint access, bool inherit, string name);
[System.Runtime.InteropServices.DllImport("kernel32.dll", SetLastError=true)]
public static extern bool SetEvent(System.IntPtr h);
'@ -EA SilentlyContinue
    # CharSet Unicode no es opcional: OpenEventW es la variante ancha y sin
    # esto .NET marshala el nombre como ANSI, la busqueda falla y parece que el
    # grabador no estuviera corriendo.
    $h = [SPV.Ev]::OpenEventW(0x0002, $false, 'Global\rice-shadowplay-cut')
    if ($h -eq [IntPtr]::Zero) { throw 'No pude senalizar el corte: el grabador no esta corriendo.' }
    [void][SPV.Ev]::SetEvent($h)
    Start-Sleep -Seconds 4
}

# Ordenar por los .mp4, NUNCA por los .pcm.
#
# dump_ring vuelca los doce .pcm de golpe en el corte, asi que todos quedan con
# la misma marca de tiempo y ordenarlos por ella da un orden arbitrario: el
# audio sale troceado y fuera de secuencia, y al escucharlo parece que hubiera
# un desfase enorme cuando lo unico que pasa es que los pedazos estan
# barajados. Los .mp4 se cierran de a uno, asi que su mtime SI es el instante
# real de cada segmento. Es el mismo criterio que usa shadowplay-wgc-save.ps1.
$videos = @(Get-ChildItem (Join-Path $buf 'seg*.mp4') -EA SilentlyContinue |
            Where-Object { $_.Length -gt 100KB } | Sort-Object LastWriteTime)
if (-not $videos) { throw "No hay nada en $buf. Corre sin -SinCorte para forzar un corte." }
$segs = @($videos | ForEach-Object { Get-Item (Join-Path $buf ($_.BaseName + '.pcm')) -EA SilentlyContinue })

New-Item -ItemType Directory -Force -Path $salida | Out-Null
$sis = Join-Path $salida 'sistema.pcm'
$mic = Join-Path $salida 'microfono.pcm'
Remove-Item $sis, $mic -Force -EA SilentlyContinue

# Concatenar en orden de tiempo, igual que hace el guardado real.
foreach ($s in $segs) {
    Get-Content $s.FullName -AsByteStream -Raw | Add-Content $sis -AsByteStream
    $m = Join-Path $buf ($s.BaseName + '.mic.pcm')
    if (Test-Path $m) { Get-Content $m -AsByteStream -Raw | Add-Content $mic -AsByteStream }
}

foreach ($par in @(@{p=$sis; n='sistema'}, @{p=$mic; n='microfono'})) {
    if (-not (Test-Path $par.p)) { Write-Host "$($par.n): NO SE CAPTURO NADA" -ForegroundColor Red; continue }
    $wav = Join-Path $salida "$($par.n).wav"
    & $ff -hide_banner -loglevel error -f s16le -ar 48000 -ac 2 -i $par.p -y $wav
    $vol = & $ff -hide_banner -f s16le -ar 48000 -ac 2 -i $par.p -af volumedetect -f null - 2>&1 |
           Select-String 'mean_volume|max_volume' | ForEach-Object { $_.Line -replace '.*\]\s*', '' }
    $seg = [math]::Round((Get-Item $par.p).Length / 4 / 48000, 1)
    Write-Host ""
    Write-Host "$($par.n): $seg s" -ForegroundColor Cyan
    $vol | ForEach-Object { Write-Host "  $_" }
    Remove-Item $par.p -Force -EA SilentlyContinue
}

Write-Host ""
Write-Host "WAV en: $salida" -ForegroundColor Green
Write-Host "Referencia: hablar de cerca da picos de -6 a -20 dB. Por debajo de"
Write-Host "-35 dB de pico, la mezcla con el audio del sistema lo entierra."
explorer $salida
