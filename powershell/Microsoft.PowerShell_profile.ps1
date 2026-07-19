# --- fastfetch on WezTerm startup ---
# Show custom duck fastfetch only for the top-level interactive WezTerm shell.
# Guards: must be inside WezTerm, an interactive console host, and not already shown
# (so nested pwsh / scripts / editors don't re-trigger it).
if ($env:WEZTERM_PANE -and
    -not $env:FASTFETCH_SHOWN -and
    $Host.Name -eq 'ConsoleHost' -and
    [Environment]::UserInteractive) {
    $env:FASTFETCH_SHOWN = '1'
    if (Get-Command fastfetch -ErrorAction SilentlyContinue) {
        fastfetch
    }
}

# --- admin: run a command elevated in a NEW window that stays open ---
# Windows `sudo` (new-window mode) closes the window when the command ends, so
# wrap the target in `pwsh -NoExit` to keep the output visible.
#   admin                       -> interactive elevated pwsh
#   admin Restart-Service sshd  -> runs it, window stays open with output
# Requires `sudo` enabled: Settings > System > For developers > Enable sudo.
function admin {
    if ($args.Count -eq 0) {
        sudo pwsh
    } else {
        sudo pwsh -NoExit -Command ($args -join ' ')
    }
}
