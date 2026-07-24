# glazewm-dwindle.ps1
# Fibonacci / dwindle-spiral auto-tiling for GlazeWM.
# GlazeWM has no native fibonacci: new windows all split the same way.
# This listens to GlazeWM's IPC and, on every focus/new-window event, sets the
# focused container's tiling direction from its shape:
#   wider than tall  -> horizontal (next window splits left/right)
#   taller than wide -> vertical   (next window splits top/bottom)
# Alternating like that produces the fibonacci spiral, so every NEW window keeps
# the pattern automatically until the user changes it (alt+v / manual resize).

# --- single instance (robust: survives a previous instance dying uncleanly) ---
$createdNew = $false
$mutex = [System.Threading.Mutex]::new($false, 'Global\glazewm-dwindle-ps', [ref]$createdNew)
try { if (-not $mutex.WaitOne(0)) { return } } catch [System.Threading.AbandonedMutexException] { }

# 127.0.0.1, not 'localhost': localhost resolves ::1 first, which GlazeWM's IPC
# doesn't listen on, so every connect burns a ~2.1s IPv6 timeout before falling
# back to IPv4 (the same trap that made the supervisor kill GlazeWM every 60s).
$uri = [Uri]'ws://127.0.0.1:6123'

# Release physical RAM while idle: this script mostly waits for events, so we
# trim the working set (pages go to standby, fault back in on demand). Drops the
# footprint from ~200 MB to ~30-40 MB.
Add-Type -Namespace Win -Name Mem -MemberDefinition '[System.Runtime.InteropServices.DllImport("psapi.dll")] public static extern bool EmptyWorkingSet(System.IntPtr h);' -ErrorAction SilentlyContinue
function Trim { try { [Win.Mem]::EmptyWorkingSet([System.Diagnostics.Process]::GetCurrentProcess().Handle) | Out-Null } catch {} }

# Bounded send. An unbounded .Wait() here could block the event loop forever if
# GlazeWM stopped draining the socket, and the supervisor only checks that this
# process exists -- so a wedge looked healthy and fibonacci layout just quietly
# stopped working. Returns $false so callers can drop out and reconnect.
function Send-WS($ws, $msg) {
  $b = [Text.Encoding]::UTF8.GetBytes($msg)
  $seg = [System.ArraySegment[byte]]::new($b)
  try { return $ws.SendAsync($seg, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, [Threading.CancellationToken]::None).Wait(3000) }
  catch { return $false }
}
# Blocking receive with its own buffer; returns text, or $null if the wait times
# out (the pending task is passed back in/out so it is never leaked).
function New-WS($uri) {
  $ws = New-Object System.Net.WebSockets.ClientWebSocket
  try { $ws.ConnectAsync($uri, [Threading.CancellationToken]::None).Wait(4000) | Out-Null } catch {}
  return $ws
}
# One-shot request/response on the action socket.
function Query-WS($ws, $msg, $timeoutMs) {
  if (-not (Send-WS $ws $msg)) { return $null }
  $buf = New-Object byte[] 131072
  $seg = [System.ArraySegment[byte]]::new($buf)
  $r = $ws.ReceiveAsync($seg, [Threading.CancellationToken]::None)
  if ($r.Wait($timeoutMs)) { return [Text.Encoding]::UTF8.GetString($buf, 0, $r.Result.Count) }
  return $null
}

while ($true) {
  $evt = $null; $act = $null
  try {
    $evt = New-WS $uri
    $act = New-WS $uri
    if ($evt.State -ne 'Open' -or $act.State -ne 'Open') { Start-Sleep 2; continue }

    # Only react to NEW windows, NOT focus changes. Reacting to focus_changed
    # made moving/focusing existing windows re-set the tiling direction, which
    # re-tiled the workspace and changed window sizes. window_managed alone still
    # produces the fibonacci spiral as windows are added, and leaves manual
    # moves/resizes untouched.
    # If the subscribe itself doesn't land there is nothing to listen to; drop out
    # and let the outer loop reconnect rather than blocking on a dead socket.
    if (-not (Send-WS $evt 'sub -e window_managed')) { Start-Sleep 2; continue }
    Start-Sleep -Milliseconds 500; Trim

    $buf = New-Object byte[] 131072
    $seg = [System.ArraySegment[byte]]::new($buf)
    $recv = $null
    while ($evt.State -eq 'Open') {
      if ($null -eq $recv) { $recv = $evt.ReceiveAsync($seg, [Threading.CancellationToken]::None) }
      # Re-wait on the SAME task each loop so nothing is leaked; a faulted task
      # (GlazeWM died) completes the wait and drops us into reconnect.
      if (-not $recv.Wait(30000)) { Trim; continue }
      if ($recv.IsFaulted -or $recv.IsCanceled) { break }
      $msg = [Text.Encoding]::UTF8.GetString($buf, 0, $recv.Result.Count)
      $recv = $null
      if ($msg -notmatch '"messageType":"event') { continue }

      $resp = Query-WS $act 'query windows' 2000
      if (-not $resp) { continue }
      try { $j = $resp | ConvertFrom-Json } catch { continue }
      $f = $j.data.windows | Where-Object { $_.hasFocus } | Select-Object -First 1
      if (-not $f) { continue }
      if ($f.width -gt $f.height) { $dir = 'horizontal' }
      elseif ($f.width -lt $f.height) { $dir = 'vertical' }
      else { continue }
      Query-WS $act "command set-tiling-direction $dir" 1000 | Out-Null
      if ($env:DWINDLE_LOG) {
        "$([DateTime]::Now.ToString('HH:mm:ss')) $($f.width)x$($f.height) -> $dir" | Add-Content -Path $env:DWINDLE_LOG
      }
    }
  } catch { Start-Sleep 2 }
  finally {
    try { $evt.Dispose() } catch {}
    try { $act.Dispose() } catch {}
  }
}
