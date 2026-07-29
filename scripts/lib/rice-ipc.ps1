# One GlazeWM IPC client for every PowerShell script in the rice.
#
# This replaces four hand-rolled copies that had drifted apart in ways that were
# actual bugs, not just noise:
#   * three still used 'localhost' (a ~2.1s IPv6 timeout per connect) after the
#     fix landed in the other two;
#   * only one read multi-frame responses, so the others silently truncated a
#     large window tree;
#   * only one used a finally, so the rest leaked the socket on any throw;
#   * one had no send timeout at all and could block its caller forever.
#
# Requires rice-paths.ps1 (for $Rice.IpcUri) to be dot-sourced first.

function Connect-GlazeIpc {
    param([int]$TimeoutMs = 4000)
    $ws = New-Object System.Net.WebSockets.ClientWebSocket
    try {
        if ($ws.ConnectAsync($Rice.IpcUri, [Threading.CancellationToken]::None).Wait($TimeoutMs)) { return $ws }
    } catch { }
    try { $ws.Dispose() } catch { }
    return $null
}

function Close-GlazeIpc {
    param($Socket, [int]$TimeoutMs = 1000)
    if (-not $Socket) { return }
    # CloseAsync rather than a bare Dispose, so GlazeWM sees a clean close instead
    # of an abortive reset on every one of the supervisor's ~2880 daily probes.
    try { [void]$Socket.CloseAsync('NormalClosure', '', [Threading.CancellationToken]::None).Wait($TimeoutMs) } catch { }
    try { $Socket.Dispose() } catch { }
}

function Send-GlazeIpc {
    param($Socket, [Parameter(Mandatory)][string]$Message, [int]$TimeoutMs = 2000)
    if (-not $Socket) { return $false }
    try {
        $b = [Text.Encoding]::UTF8.GetBytes($Message)
        $seg = [System.ArraySegment[byte]]::new($b)
        return $Socket.SendAsync($seg, 'Text', $true, [Threading.CancellationToken]::None).Wait($TimeoutMs)
    } catch { return $false }
}

function Receive-GlazeIpc {
    param($Socket, [int]$TimeoutMs = 3000)
    if (-not $Socket) { return $null }
    try {
        $buf = New-Object byte[] 131072
        $seg = [System.ArraySegment[byte]]::new($buf)
        $sb = New-Object Text.StringBuilder
        # Loop until EndOfMessage: GlazeWM's window tree does not fit one frame.
        do {
            $r = $Socket.ReceiveAsync($seg, [Threading.CancellationToken]::None)
            if (-not $r.Wait($TimeoutMs)) { return $null }
            [void]$sb.Append([Text.Encoding]::UTF8.GetString($buf, 0, $r.Result.Count))
        } while (-not $r.Result.EndOfMessage)
        return $sb.ToString()
    } catch { return $null }
}

# Connect, send, read, close -- always in a finally.
#
# El plazo es para la operacion ENTERA, no por fase. Antes -TimeoutMs solo
# llegaba a Connect-GlazeIpc y las otras tres fases traian el suyo propio
# (Send 2000 + Receive 3000 + Close 1000), asi que una sonda que conectaba y
# luego se colgaba costaba ~8 s aunque se hubieran pedido 2. Eso multiplicaba
# por cuatro el presupuesto de Wait-GlazeIpcReady, que es la funcion de la que
# dependen el arranque de sesion (rice-autostart) y el supervisor: -TimeoutSec 90
# podia convertirse en ~98 s reales con todo parado esperando.
function Invoke-GlazeIpc {
    param([Parameter(Mandatory)][string]$Command, [int]$TimeoutMs = 4000, [switch]$Raw)
    $sw = [Diagnostics.Stopwatch]::StartNew()
    # Suelo de 200 ms: agotado el presupuesto sigue haciendo falta un plazo
    # positivo para que el cierre no quede colgado sin limite.
    $left = { [int][Math]::Max(200, $TimeoutMs - $sw.ElapsedMilliseconds) }

    $ws = Connect-GlazeIpc -TimeoutMs (& $left)
    if (-not $ws) { return $null }
    try {
        if (-not (Send-GlazeIpc $ws $Command -TimeoutMs (& $left))) { return $null }
        $txt = Receive-GlazeIpc $ws -TimeoutMs (& $left)
        if (-not $txt) { return $null }
        if ($Raw) { return $txt }
        try { return $txt | ConvertFrom-Json } catch { return $null }
    } finally { Close-GlazeIpc $ws -TimeoutMs (& $left) }
}

# Is GlazeWM responding? Uses `query focused` rather than `query windows`: the
# old probe made GlazeWM serialize the whole window tree only to read 2048 bytes
# of it and throw the rest away.
function Test-GlazeIpcAlive {
    param([int]$TimeoutMs = 4000)
    $null -ne (Invoke-GlazeIpc -Command 'query focused' -TimeoutMs $TimeoutMs)
}

# Block until GlazeWM answers, or give up. Replaces the fixed login sleeps that
# were guesses in both the supervisor and the autostart script.
function Wait-GlazeIpcReady {
    param([int]$TimeoutSec = 60)
    $end = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $end) {
        if (Test-GlazeIpcAlive -TimeoutMs 2000) { return $true }
        Start-Sleep -Milliseconds 700
    }
    return $false
}

function Get-GlazeMonitors {
    (Invoke-GlazeIpc -Command 'query monitors').data.monitors
}

# Rect of the monitor that currently has focus; falls back to the first monitor.
function Get-GlazeFocusedMonitor {
    $mons = Get-GlazeMonitors
    if (-not $mons) { return $null }
    $m = $mons | Where-Object { $_.hasFocus } | Select-Object -First 1
    if (-not $m) { $m = $mons | Select-Object -First 1 }
    @{ x = [int]$m.x; y = [int]$m.y; width = [int]$m.width; height = [int]$m.height }
}

function Set-GlazeWorkspace {
    param([Parameter(Mandatory)]$Index, [int]$SettleMs = 400)
    $ok = $null -ne (Invoke-GlazeIpc -Command "command focus --workspace $Index")
    if ($ok -and $SettleMs -gt 0) { Start-Sleep -Milliseconds $SettleMs }
    return $ok
}
