# Stitch the last ~30s of the WGC rolling buffer into a replay clip.
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

$ts = Get-Date -Format 'yyyyMMdd_HHmmss'
$dest = Join-Path $out "replay_$ts.mp4"
& $ff -hide_banner -loglevel error -f concat -safe 0 -i $listFile -c copy -y $dest 2>$null

if (Test-Path $dest) {
    # Toast on the FOCUSED monitor (per GlazeWM IPC), fall back to primary top-right.
    $nx = 1490; $ny = 50
    try {
        $sock = New-Object System.Net.WebSockets.ClientWebSocket
        $ct = [System.Threading.CancellationToken]::None
        $connected = $sock.ConnectAsync([Uri]'ws://localhost:6123', $ct).Wait(4000)
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
    Start-Process 'C:\Users\obisp\dev\glaze-bar\target\release\shadowplay-notify.exe' -ArgumentList "`"$dest`"", $nx, $ny
    Write-Output $dest
}
