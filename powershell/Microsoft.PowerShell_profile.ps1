# --- fastfetch on WezTerm startup ---
# Show custom duck fastfetch only for the top-level interactive WezTerm shell.
# Guards: must be inside WezTerm, an interactive console host, and not already shown
# (so nested pwsh / scripts / editors don't re-trigger it).
# --- fastfetch on WezTerm startup ---
if ($env:WEZTERM_PANE -and
    -not $env:FASTFETCH_SHOWN -and
    $Host.Name -eq 'ConsoleHost' -and
    [Environment]::UserInteractive) {
    $env:FASTFETCH_SHOWN = '1'
    # Print the cached render instantly (~1ms) so the prompt isn't held up by
    # fastfetch's ~130ms, then refresh the cache in the background for next time.
    # `--pipe false` keeps the ANSI colours when writing to the file. Dynamic
    # fields (uptime/mem) show last-open's values -> refreshed each launch.
    $ffCache = "$env:LOCALAPPDATA\ff-cache.txt"
    if (Test-Path $ffCache) {
        Write-Host -NoNewline (Get-Content $ffCache -Raw)
        $ffStale = ((Get-Date) - (Get-Item $ffCache).LastWriteTime).TotalMinutes -gt 5
    } else {
        if (Get-Command fastfetch -ErrorAction SilentlyContinue) { fastfetch }
        $ffStale = $true
    }
    # Only regenerate when the cache is missing or >5 min old, so rapid shell
    # opens just print the cache (~16ms) instead of paying the ~100ms respawn.
    if ($ffStale) {
        Start-Process fastfetch -NoNewWindow -ArgumentList '--pipe', 'false' -RedirectStandardOutput $ffCache -EA SilentlyContinue
    }
}

# --- Quick Admin WezTerm Launcher ---
function Set-AdminWezTerm {
    # Creamos un bloque de configuración de inicio limpio
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "wezterm-gui"
    $psi.Verb = "RunAs"
    
    # Eliminamos la variable heredada para que la nueva ventana sí muestre fastfetch
    if ($psi.EnvironmentVariables.ContainsKey("FASTFETCH_SHOWN")) {
        $psi.EnvironmentVariables.Remove("FASTFETCH_SHOWN")
    }
    
    [System.Diagnostics.Process]::Start($psi) > $null
}
Set-Alias -Name swez -Value Set-AdminWezTerm

# --- admin: run a command elevated in a NEW window that stays open ---
# sudo (new-window mode) closes the window when the command ends, so wrap the
# target in `pwsh -NoExit` to keep the output visible.
#   admin                       -> interactive elevated pwsh
#   admin Restart-Service sshd  -> runs it, window stays open with output
function admin {
    if ($args.Count -eq 0) {
        sudo pwsh
    } else {
        sudo pwsh -NoExit -Command ($args -join ' ')
    }
}

# cava: terminal audio spectrum visualizer (system audio -> FFT bars, 165fps).
# Quit with q / Esc / Ctrl+C.
function cava { & "$HOME\dev\target\release\cava.exe" @args }

# notify-test: preview the caelestia toast to iterate on its look.
#   notify-test "Título" "cuerpo" mic "#e0a35c"      (icon: mic|replay|check|info|warn)
function notify-test {
    param(
        [string]$Title = 'Prueba de notificación',
        [string]$Body = 'cuerpo de ejemplo',
        [string]$Icon = 'info',
        [string]$Accent = '#e0a35c',
        [int]$X = 100, [int]$Y = 100, [double]$Hold = 6
    )
    $exe = "$HOME\dev\target\release\shadowplay-notify.exe"
    Start-Process $exe -ArgumentList "--title `"$Title`" --body `"$Body`" --icon $Icon --accent `"$Accent`" --x $X --y $Y --hold $Hold"
}

# mic: cycle the active mic (HyperX <-> Snowball) and toast the new one. The
# recorder's mic capture auto-follows the new default within ~2s.
function mic {
    $name = & "$HOME\dev\target\release\micswitch.exe" 2>$null
    if (-not $name) { Write-Host 'No hay HyperX/Snowball activo'; return }
    $short = $name -replace '^Micr[oó]fono \(', '' -replace '\)$', ''
    @{icon = 'mic'; title = 'Micrófono'; body = $short; accent = '#e0a35c' } |
        ConvertTo-Json -Compress | Set-Content "$HOME\.config\island.json" -Encoding utf8
    Start-Process "$HOME\dev\target\release\shadowplay-notify.exe" `
        -ArgumentList "--title `"Micrófono`" --body `"$short`" --icon mic --accent `"#e0a35c`" --hold 4"
}

# island-test: push an event to the in-bar dynamic island to iterate on its look.
#   island-test "Título" "cuerpo" mic "#e0a35c"      (icon: mic|replay|check|rec|info|warn)
function island-test {
    param([string]$Title = 'Prueba', [string]$Body = 'cuerpo de ejemplo', [string]$Icon = 'info', [string]$Accent = '#e0a35c')
    @{icon = $Icon; title = $Title; body = $Body; accent = $Accent } |
        ConvertTo-Json -Compress | Set-Content "$HOME\.config\island.json" -Encoding utf8
}

