# Process liveness + component launching for the supervisor.
#
# The supervisor previously used four different liveness idioms inside one loop
# (Get-Process, Get-Process|Measure, a WMI Get-CimInstance command-line match,
# and an IPC probe), and five different Start-Process shapes, only two of which
# checked that the target existed first.

# ONE process enumeration per tick, then O(1) lookups. The old loop walked the
# process table eight times per tick.
function Get-RiceProcessSnapshot {
    $set = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($p in (Get-Process -EA SilentlyContinue)) { [void]$set.Add($p.ProcessName) }
    # The leading comma is load-bearing: PowerShell unrolls a returned collection
    # into an Object[], which silently drops the OrdinalIgnoreCase comparer and
    # makes Contains() case-sensitive. That made 'GlazeWM' fail to match the
    # process actually named 'glazewm', so the supervisor believed the WM was
    # dead and launched another copy every single tick.
    return , $set
}

function Test-RiceAlive {
    param([Parameter(Mandatory)]$Snapshot, [Parameter(Mandatory)][string]$Name)
    $Snapshot.Contains($Name)
}

function Get-RiceAliveCount {
    param([Parameter(Mandatory)][string]$Name)
    @(Get-Process $Name -EA SilentlyContinue).Count
}

# Is a named mutex held? Sub-millisecond, and it identifies a *specific* script
# rather than "some pwsh.exe whose command line matches" -- which is what the WMI
# query was doing, at ~50-200ms and a WmiPrvSE spin-up every 30 seconds.
function Test-RiceMutex {
    param([Parameter(Mandatory)][string]$Name)
    $handle = $null
    try {
        if ([System.Threading.Mutex]::TryOpenExisting($Name, [ref]$handle)) {
            $handle.Dispose()
            return $true
        }
        return $false
    } catch { return $false }
}

function Write-RiceLog {
    param([Parameter(Mandatory)][string]$Message, [string]$Component = 'supervisor')
    try {
        if (-not (Test-Path $Rice.LogDir)) { New-Item -ItemType Directory -Force $Rice.LogDir | Out-Null }
        $log = Join-Path $Rice.LogDir 'supervisor.log'
        # Keep it bounded: this runs forever.
        if ((Test-Path $log) -and (Get-Item $log).Length -gt 256KB) {
            $keep = Get-Content $log -Tail 500
            Set-Content -Path $log -Value $keep -Encoding utf8
        }
        "$([DateTime]::Now.ToString('yyyy-MM-dd HH:mm:ss')) [$Component] $Message" | Add-Content -Path $log -Encoding utf8
    } catch { }
}

# Launch a component described by the supervisor's table. Always checks the
# target exists, always suppresses a launch error into the log rather than a
# hidden console's stderr.
function Start-RiceComponent {
    param([Parameter(Mandatory)]$Component)
    $path = & $Component.Path
    $isShell = $path -like 'shell:*'
    if (-not $isShell -and -not (Test-Path $path)) {
        Write-RiceLog "missing binary: $path" $Component.Name
        return $false
    }
    try {
        $argv = if ($Component.Args) { & $Component.Args } else { @() }
        # -WindowStyle Hidden on Start-Process itself, NOT just inside the child's
        # own arguments. Passing pwsh `-WindowStyle Hidden` only hides PowerShell's
        # window; Windows still allocates a console for the new process, and since
        # the default terminal here is Windows Terminal that surfaces as a WT
        # window popping open and stealing focus. `shell:` AppsFolder targets don't
        # accept the switch, so they are launched plainly.
        if ($isShell) {
            Start-Process $path -EA Stop
        } elseif ($argv -and $argv.Count) {
            Start-Process $path -ArgumentList $argv -WindowStyle Hidden -EA Stop
        } else {
            Start-Process $path -WindowStyle Hidden -EA Stop
        }
        Write-RiceLog "started" $Component.Name
        return $true
    } catch {
        Write-RiceLog "start failed: $_" $Component.Name
        return $false
    }
}

# Decide and act for one component. Holds the per-component state (restart
# counters, grace windows) that used to be an ad-hoc $glzFails variable covering
# only GlazeWM.
function Step-RiceComponent {
    param([Parameter(Mandatory)]$Component, [Parameter(Mandatory)]$Snapshot, [Parameter(Mandatory)][hashtable]$State)

    $name = $Component.Name
    if (-not $State.ContainsKey($name)) {
        $State[$name] = @{ Restarts = 0; Fails = 0; StartedAt = [DateTime]::MinValue }
    }
    $st = $State[$name]

    $running = switch ($Component.Check) {
        'Mutex' { Test-RiceMutex $Component.Match }
        default { Test-RiceAlive $Snapshot $Component.Match }
    }

    if (-not $running) {
        $cap = if ($null -ne $Component.MaxRestarts) { $Component.MaxRestarts } else { 0 }
        if ($cap -gt 0 -and $st.Restarts -ge $cap) { return }   # stop retrying what never comes up
        if (Start-RiceComponent $Component) {
            $st.Restarts++
            $st.StartedAt = Get-Date
            $st.Fails = 0
        }
        return
    }

    # Running. Some components can be alive but wedged -- probe them, after a
    # grace window so a component that just started isn't judged while warming up.
    if (-not $Component.Health) { $st.Fails = 0; return }
    $grace = if ($null -ne $Component.Grace) { $Component.Grace } else { 60 }
    if ($st.StartedAt -ne [DateTime]::MinValue -and ((Get-Date) - $st.StartedAt).TotalSeconds -lt $grace) { return }

    if (& $Component.Health) { $st.Fails = 0; return }

    $st.Fails++
    $need = if ($null -ne $Component.Fails) { $Component.Fails } else { 3 }
    if ($st.Fails -lt $need) { return }

    Write-RiceLog "unhealthy for $($st.Fails) checks -> restarting" $name
    # Kill propio cuando el componente lo trae. Hace falta siempre que Match no
    # sea un nombre de proceso -- con Check='Mutex' lo que hay ahi es el nombre
    # de un mutex, y `Get-Process` sobre eso no mata nada: la instancia vieja
    # seguiria parada, y la nueva se suicidaria al no poder tomar el mutex.
    if ($Component.Kill) { & $Component.Kill }
    else { Get-Process $Component.Match -EA SilentlyContinue | Stop-Process -Force -EA SilentlyContinue }
    Start-Sleep -Milliseconds 600
    if (Start-RiceComponent $Component) { $st.StartedAt = Get-Date }
    $st.Fails = 0
}
