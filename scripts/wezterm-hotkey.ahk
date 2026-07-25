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
; Stop a BARE Win press from opening the Start menu, without breaking any
; Win+... combination.
;
; Windows opens Start on Win *keyup* only when no other key was pressed in
; between. So: `~` passes the real LWin through (GlazeWM's lwin+... bindings and
; CmdPal's Win+Ctrl+Space still see it), and we immediately send vkE8 -- an
; unassigned virtual key that does nothing -- so the OS sees a combination and
; leaves Start alone.
;
; Remapping LWin outright would have been simpler and wrong: GlazeWM listens for
; the physical LWin, so the entire keybind set would have died with it.
; ------------------------------------------------------------
~LWin::Send '{Blind}{vkE8}'

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
; Win+Shift+B -> show/hide the Windows taskbar.
; Auto-hide alone still slides it back in on hover; the tool combines that (which
; is what frees the work area for tiling) with hiding the window outright.
; ------------------------------------------------------------
#+b:: {
    Run('"' . EnvGet('USERPROFILE') . '\dev\target\release\taskbar.exe" --toggle', , 'Hide')
}

; ------------------------------------------------------------
; ShadowPlay: Alt+F10 saves the last ~30s from the rolling buffer.
; ------------------------------------------------------------
!F10:: {
    ; Save runs the concat then pops the custom Rust notification itself.
    Run('pwsh -NoProfile -WindowStyle Hidden -File "C:\Users\obisp\.config\shadowplay-wgc-save.ps1"', , 'Hide')
}
