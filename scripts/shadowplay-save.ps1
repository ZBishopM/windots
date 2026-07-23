# Stitch the last ~30s of the rolling buffer into a clip.
$buf = 'C:\Users\obisp\ShadowPlay\buffer'
$out = 'C:\Users\obisp\ShadowPlay\clips'
$ff = 'C:\Users\obisp\scoop\shims\ffmpeg.exe'
New-Item -ItemType Directory -Force -Path $out | Out-Null

$segs = Get-ChildItem "$buf\seg*.mkv" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime
if ($segs.Count -lt 2) { exit 1 }

# The newest segment is still being written (incomplete) -> drop it, then take
# the newest 6 complete ones (~30s), in chronological order.
$complete = $segs[0..($segs.Count - 2)]
$last = @($complete | Select-Object -Last 6)

$listFile = Join-Path $buf '_concat.txt'
($last | ForEach-Object { "file '" + ($_.FullName -replace "'", "''") + "'" }) |
    Set-Content -Path $listFile -Encoding ascii

$ts = Get-Date -Format 'yyyyMMdd_HHmmss'
$dest = Join-Path $out "replay_$ts.mp4"
& $ff -hide_banner -loglevel error -f concat -safe 0 -i $listFile -c copy -y $dest 2>$null

if (Test-Path $dest) {
    # Place the toast on the FOCUSED monitor (per GlazeWM), i.e. where the user is
    # actually looking. The cursor is unreliable: it only jumps on monitor-focus
    # changes, not on workspace switches within a monitor, so it drifts out of
    # sync. Query GlazeWM's IPC for the focused monitor; fall back to the primary
    # top-right. All monitors are 100% scale -> virtual pixels map 1:1.
    $nx = 1490; $ny = 50
    try {
        $sock = New-Object System.Net.WebSockets.ClientWebSocket
        $ct = [System.Threading.CancellationToken]::None
        # Connect can take >2s on a cold call; give it room, and only query if it
        # actually opened. [void] on the Wait() calls keeps their bool off stdout.
        $connected = $sock.ConnectAsync([Uri]'ws://localhost:6123', $ct).Wait(4000)
        if ($connected) {
            $q = [Text.Encoding]::UTF8.GetBytes('query monitors')
            [void]$sock.SendAsync((New-Object System.ArraySegment[byte] (, $q)), 'Text', $true, $ct).Wait(3000)
            $buf = New-Object byte[] 131072
            $rseg = New-Object System.ArraySegment[byte] (, $buf)
            $sb = New-Object Text.StringBuilder
            do {
                $r = $sock.ReceiveAsync($rseg, $ct); [void]$r.Wait(3000)
                [void]$sb.Append([Text.Encoding]::UTF8.GetString($buf, 0, $r.Result.Count))
            } while (-not $r.Result.EndOfMessage)
            $sock.Dispose()
            $mon = ($sb.ToString() | ConvertFrom-Json).data.monitors | Where-Object { $_.hasFocus } | Select-Object -First 1
            if ($mon) {
                $nx = [int]($mon.x + $mon.width - 420 - 10)   # toast width 420 + 10px right margin
                $ny = [int]($mon.y + 50)
            }
        }
    } catch {}
    Start-Process 'C:\Users\obisp\dev\target\release\shadowplay-notify.exe' -ArgumentList "`"$dest`"", $nx, $ny
    Write-Output $dest
}
