# Stitch the last ~30s of the WGC rolling buffer into a replay clip.
# Video segments (segNN.mp4) are video-only; each has parallel raw-PCM audio in
# lockstep: segNN.pcm (system audio) and segNN.mic.pcm (microphone), s16le 48k
# stereo. We concat the video + each audio ring; if the mic ring exists we mix
# system + mic into one track, otherwise system audio only.
$buf = 'C:\Users\obisp\ShadowPlay\wgc-buffer'
$out = 'C:\Users\obisp\ShadowPlay\clips'
$ff  = 'C:\Users\obisp\scoop\apps\ffmpeg\current\bin\ffmpeg.exe'
New-Item -ItemType Directory -Force -Path $out | Out-Null

$segs = Get-ChildItem "$buf\seg*.mp4" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime
if ($segs.Count -lt 2) { exit 1 }

# Newest segment is still being written -> drop it, take the newest 6 (~30s).
$complete = $segs[0..($segs.Count - 2)]
$last = @($complete | Select-Object -Last 6)

$listFile = Join-Path $buf '_concat.txt'
($last | ForEach-Object { "file '" + ($_.FullName -replace "'", "''") + "'" }) |
    Set-Content -Path $listFile -Encoding ascii

# Concat the matching per-segment PCM (same time order): system audio and mic.
$sysOut = Join-Path $buf '_sys.pcm'
$micOut = Join-Path $buf '_mic.pcm'
$haveSys = $true
$haveMic = $true
$fsS = [System.IO.File]::Create($sysOut)
$fsM = [System.IO.File]::Create($micOut)
foreach ($v in $last) {
    $ps = Join-Path $buf ($v.BaseName + '.pcm')
    $pm = Join-Path $buf ($v.BaseName + '.mic.pcm')
    if (Test-Path $ps) { $b = [System.IO.File]::ReadAllBytes($ps); $fsS.Write($b, 0, $b.Length) } else { $haveSys = $false }
    if (Test-Path $pm) { $b = [System.IO.File]::ReadAllBytes($pm); $fsM.Write($b, 0, $b.Length) } else { $haveMic = $false }
}
$fsS.Close(); $fsM.Close()
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
Remove-Item $sysOut, $micOut -ErrorAction SilentlyContinue

if (Test-Path $dest) {
    # Toast on the FOCUSED monitor (per GlazeWM IPC), fall back to primary top-right.
    $nx = 1490; $ny = 50
    try {
        $sock = New-Object System.Net.WebSockets.ClientWebSocket
        $ct = [System.Threading.CancellationToken]::None
        $connected = $sock.ConnectAsync([Uri]'ws://127.0.0.1:6123', $ct).Wait(4000)
        if ($connected) {
            $q = [Text.Encoding]::UTF8.GetBytes('query monitors')
            [void]$sock.SendAsync((New-Object System.ArraySegment[byte] (, $q)), 'Text', $true, $ct).Wait(3000)
            $rbuf = New-Object byte[] 131072
            $rseg = New-Object System.ArraySegment[byte] (, $rbuf)
            $sb = New-Object Text.StringBuilder
            do {
                $r = $sock.ReceiveAsync($rseg, $ct); [void]$r.Wait(3000)
                [void]$sb.Append([Text.Encoding]::UTF8.GetString($rbuf, 0, $r.Result.Count))
            } while (-not $r.Result.EndOfMessage)
            $sock.Dispose()
            $mon = ($sb.ToString() | ConvertFrom-Json).data.monitors | Where-Object { $_.hasFocus } | Select-Object -First 1
            if ($mon) {
                $nx = [int]($mon.x + $mon.width - 420 - 10)
                $ny = [int]($mon.y + 50)
            }
        }
    } catch {}
    $fname = Split-Path $dest -Leaf
    # feed the in-bar dynamic island (shows when the bar is visible)
    @{icon = 'replay'; title = 'Replay guardado'; body = $fname; accent = '#a9b56a' } |
        ConvertTo-Json -Compress | Set-Content "$env:USERPROFILE\.config\island.json" -Encoding utf8
    # and the standalone toast (visible over fullscreen games, where the bar is hidden)
    $nargs = "--title `"Replay guardado`" --body `"$fname`" --icon replay --accent `"#a9b56a`" --open `"$dest`" --x $nx --y $ny"
    Start-Process 'C:\Users\obisp\dev\target\release\shadowplay-notify.exe' -ArgumentList $nargs
    Write-Output $dest
}
