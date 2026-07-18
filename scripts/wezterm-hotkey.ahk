#Requires AutoHotkey v2.0
#SingleInstance Force
; ------------------------------------------------------------
; Hyprland-style global hotkey to launch WezTerm.
;   #  = SUPER (Windows key)
;   SUPER + Enter  -> open a new WezTerm window
; Runs from the Startup folder, so the hotkey is always active.
; ------------------------------------------------------------

#Enter:: {
    Run '"C:\Program Files\WezTerm\wezterm-gui.exe" start'
}

; ------------------------------------------------------------
; ShadowPlay: Alt+F10 saves the last ~30s from the rolling buffer.
; ------------------------------------------------------------
!F10:: {
    ; Save runs the concat then pops the custom Rust notification itself.
    Run('pwsh -NoProfile -WindowStyle Hidden -File "C:\Users\obisp\.config\shadowplay-save.ps1"', , 'Hide')
}
