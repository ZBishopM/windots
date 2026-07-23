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

