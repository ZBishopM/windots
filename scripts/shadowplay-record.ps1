# ShadowPlay rolling recorder with auto-detected microphone.
# Video: ddagrab -> AV1 NVENC. Audio: WASAPI loopback (system) mixed with the
# connected mic (Blue Snowball preferred, else HyperX, else any real mic). The
# mic is OPTIONAL: if none is connected it records video + system audio only, and
# re-detects on each restart. Uses .NET processes so the binary loopback->ffmpeg
# pipe and the accented dshow device names ("Micrófono ...") are handled cleanly.

# Use the REAL ffmpeg, not the scoop shim: the shim launches ffmpeg as a child,
# so killing the shim (what $ffProc.Kill() does) orphans the real encoder. The
# watchdog must be able to actually stop it, or orphaned ffmpeg instances pile up
# and collide on the same segment files (choppy audio, buffer never fills).
$ff  = 'C:\Users\obisp\scoop\apps\ffmpeg\current\bin\ffmpeg.exe'
$lb  = 'C:\Users\obisp\dev\glaze-bar\target\release\sysaudio-loopback.exe'
$buf = 'C:\Users\obisp\ShadowPlay\buffer'
$prefer = @('Blue Snowball', 'HyperX')          # priority order
$exclude = 'Oculus|NVIDIA|Steam|CABLE|VoiceMeeter|Mezcla|Stereo Mix'  # not real mics

# Mic OFF by default. A dshow mic can't share a clock with ddagrab, so mixing it
# in real time drags the video into heavy frame dup/drop and drifts the audio
# ~500ms -> choppy A/V. Video + system-audio (loopback) alone is smooth. Set
# $useMic = $true only if you accept the choppiness (needs a post-sync rework
# to do properly).
$useMic = $false

function Get-Mic {
    $out = & $ff -hide_banner -list_devices true -f dshow -i dummy 2>&1 | Out-String
    $names = [regex]::Matches($out, '"([^"]+)"\s*\(audio\)') | ForEach-Object { $_.Groups[1].Value }
    $names = $names | Where-Object { $_ -notmatch $exclude }
    foreach ($p in $prefer) {
        $hit = $names | Where-Object { $_ -match [regex]::Escape($p) } | Select-Object -First 1
        if ($hit) { return $hit }
    }
    return ($names | Select-Object -First 1)   # any remaining real mic, or $null
}

function Ffmpeg-Args($mic) {
    $a = [System.Collections.Generic.List[string]]::new()
    '-hide_banner','-loglevel','error',
    '-f','lavfi','-i','ddagrab=output_idx=0:framerate=60',
    '-f','s16le','-ar','48000','-ac','2','-i','pipe:0' | ForEach-Object { $a.Add($_) }
    if ($mic) {
        '-f','dshow','-thread_queue_size','1024','-i',"audio=$mic",
        '-filter_complex','[2:a]aresample=48000[m];[1:a][m]amix=inputs=2:duration=first:normalize=0[aout]',
        '-map','0:v:0','-map','[aout]' | ForEach-Object { $a.Add($_) }
    } else {
        '-map','0:v:0','-map','1:a:0' | ForEach-Object { $a.Add($_) }
    }
    '-c:v','av1_nvenc','-preset','p4','-tune','hq','-rc','vbr','-cq','19','-b:v','0','-g','300',
    '-c:a','aac','-b:a','160k',
    '-f','segment','-segment_format','matroska','-segment_time','5','-segment_wrap','8','-reset_timestamps','1',
    '-y',"$buf\seg%02d.mkv" | ForEach-Object { $a.Add($_) }
    return $a
}

while ($true) {
    # Start each capture from a clean buffer so a save never mixes fresh segments
    # with stale ones left behind by a previous (possibly frozen) run.
    Get-ChildItem "$buf\seg*.mkv" -EA SilentlyContinue | Remove-Item -Force -EA SilentlyContinue

    $mic = if ($useMic) { Get-Mic } else { $null }

    $pl = New-Object Diagnostics.ProcessStartInfo
    $pl.FileName = $lb; $pl.UseShellExecute = $false; $pl.RedirectStandardOutput = $true; $pl.CreateNoWindow = $true
    $lbProc = [Diagnostics.Process]::Start($pl)

    $pf = New-Object Diagnostics.ProcessStartInfo
    $pf.FileName = $ff; $pf.UseShellExecute = $false; $pf.RedirectStandardInput = $true; $pf.CreateNoWindow = $true
    (Ffmpeg-Args $mic) | ForEach-Object { $pf.ArgumentList.Add($_) }
    $ffProc = [Diagnostics.Process]::Start($pf)

    # Pump loopback stdout -> ffmpeg stdin (binary).
    $pump = $lbProc.StandardOutput.BaseStream.CopyToAsync($ffProc.StandardInput.BaseStream)

    # Watchdog: if ddagrab loses the desktop (UAC secure desktop, display/mode
    # change, GPU reset), ffmpeg does NOT exit -- it keeps running but stops
    # rotating segments, writing one file forever and freezing the buffer. The
    # newest segment normally changes every ~5s, so if the same file stays newest
    # for >16s, kill ffmpeg and let this loop start a fresh capture.
    $lastSeg = ''; $lastRotate = Get-Date
    while (-not $ffProc.HasExited) {
        Start-Sleep -Seconds 4
        $newest = Get-ChildItem "$buf\seg*.mkv" -EA SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 1
        $name = if ($newest) { $newest.Name } else { '' }
        if ($name -ne $lastSeg) { $lastSeg = $name; $lastRotate = Get-Date }
        elseif (((Get-Date) - $lastRotate).TotalSeconds -gt 16) {
            try { $ffProc.Kill() } catch {}
            break
        }
    }
    try { $lbProc.Kill() } catch {}
    try { $ffProc.StandardInput.Close() } catch {}
    Start-Sleep -Seconds 3
}
