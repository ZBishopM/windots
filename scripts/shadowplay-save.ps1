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
    Start-Process 'C:\Users\obisp\dev\glaze-bar\target\release\shadowplay-notify.exe' -ArgumentList "`"$dest`""
    Write-Output $dest
}
