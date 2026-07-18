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
