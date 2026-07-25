#Requires AutoHotkey v2.0
#SingleInstance Force
; ------------------------------------------------------------
; Hyprland-style global hotkey to launch WezTerm.
;   #  = SUPER (Windows key)
;   SUPER + Enter  -> open a new WezTerm window
; Runs from the Startup folder, so the hotkey is always active.
; ------------------------------------------------------------

#Enter:: {
    Run('"C:\Program Files\WezTerm\wezterm-gui.exe" start')
}

; ------------------------------------------------------------
; Win+Space -> Command Palette (CmdPal), NOT the OS language switch.
; Windows fires its input-language switcher on Win+Space even with a
; low-level hook, so AHK claims the combo (suppressing the switch) and
; forwards to CmdPal, which listens on Win+Ctrl+Space.
; ------------------------------------------------------------
#Space:: {
    Send '#^{Space}'
    ; Then FORCE the palette foreground instead of hoping it wins the race.
    ; Sending the hotkey alone is racy: CmdPal shows its window and calls
    ; SetForegroundWindow, but another process asserting focus at the same moment
    ; can win -- leaving the palette drawn on screen yet not foreground, so every
    ; keystroke goes to the previous window. That is the "I can't type in the
    ; palette" symptom; measured with the window visible at 800x480 while the
    ; foreground window was WezTerm.
    ; AHK just handled this keypress, so it holds foreground rights and its
    ; WinActivate is allowed to reassign focus.
    if WinWait('ahk_exe Microsoft.CmdPal.UI.exe', , 1.5) {
        if !WinActive('ahk_exe Microsoft.CmdPal.UI.exe')
            WinActivate('ahk_exe Microsoft.CmdPal.UI.exe')
    }
}

; ------------------------------------------------------------
; ShadowPlay: Alt+F10 saves the last ~30s from the rolling buffer.
; ------------------------------------------------------------
!F10:: {
    ; Save runs the concat then pops the custom Rust notification itself.
    Run('pwsh -NoProfile -WindowStyle Hidden -File "C:\Users\obisp\.config\shadowplay-wgc-save.ps1"', , 'Hide')
}
