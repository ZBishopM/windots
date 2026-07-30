# Stitch the last ~30s of the WGC rolling buffer into a replay clip.
# Video segments (segNN.mp4) are video-only; each has parallel raw-PCM audio in
# lockstep: segNN.pcm (system audio) and segNN.mic.pcm (microphone), s16le 48k
# stereo. We concat the video + each audio ring; if the mic ring exists we mix
# system + mic into one track, otherwise system audio only.
. "$env:USERPROFILE\.config\lib\rice-paths.ps1"
# (rice-ipc is no longer needed here: the toast used to be positioned on the
#  focused monitor via GlazeWM IPC, and the toast is gone.)

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

# Newest segment is still being written -> drop it, take the newest 6 (~30s).
$complete = $segs[0..($segs.Count - 2)]
$last = @($complete | Select-Object -Last 6)

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
    foreach ($v in $last) {
        $ps = Join-Path $buf ($v.BaseName + '.pcm')
        $pm = Join-Path $buf ($v.BaseName + '.mic.pcm')
        if (Test-Path $ps) { $b = [System.IO.File]::ReadAllBytes($ps); $fsS.Write($b, 0, $b.Length) } else { $haveSys = $false }
        if (Test-Path $pm) { $b = [System.IO.File]::ReadAllBytes($pm); $fsM.Write($b, 0, $b.Length) } else { $haveMic = $false }
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
    # tiene que devolver el control ya. -WindowStyle Hidden para que no parpadee
    # una consola encima del juego.
    $share = Join-Path $Rice.Config 'rice-clip-share.ps1'
    if (Test-Path $share) {
        $a = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $share, $dest)
        if ($esJuego) { $a += '-Game' }
        Start-Process pwsh -ArgumentList $a -WindowStyle Hidden
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
