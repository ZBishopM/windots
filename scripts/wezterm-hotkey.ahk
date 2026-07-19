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
; Win+Space -> Command Palette (CmdPal), NOT the OS language switch.
; Windows fires its input-language switcher on Win+Space even with a
; low-level hook, so AHK claims the combo (suppressing the switch) and
; forwards to CmdPal, which must be set to listen on Win+Ctrl+Space.
; ------------------------------------------------------------
#Space:: {
    Send '#^{Space}'
}

; ------------------------------------------------------------
; ShadowPlay: Alt+F10 saves the last ~30s from the rolling buffer.
; ------------------------------------------------------------
!F10:: {
    ; Save runs the concat then pops the custom Rust notification itself.
    Run('pwsh -NoProfile -WindowStyle Hidden -File "C:\Users\obisp\.config\shadowplay-save.ps1"', , 'Hide')
}
