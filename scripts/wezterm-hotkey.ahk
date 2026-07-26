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
    ; CmdPal's own Win+Ctrl+Space already toggles it, so just forward the combo
    ; and let it decide -- but note whether it was up first, because that decides
    ; whether the focus nudge below applies.
    wasActive := WinActive('ahk_exe Microsoft.CmdPal.UI.exe')

    ; {Blind} plus Ctrl only. Super is already physically down -- this IS a
    ; #Space hotkey -- so there is nothing to synthesise. Letting AHK build the
    ; '#' itself made it emit an LWin up afterwards, after which AHK stopped
    ; seeing Super as held at all: of four Space taps with Super held down, only
    ; the FIRST ever reached this handler. That was the "press it again and
    ; nothing happens" bug, and it was never about the palette.
    SendInput '{Blind}{Ctrl down}{Space}{Ctrl up}'

    ; Closing? Then stop here. The wait below would never match (the palette is
    ; going away), so it would park this thread for the full timeout -- and AHK
    ; runs one thread per hotkey by default, so a press arriving meanwhile is
    ; DISCARDED rather than queued. Worse, the WinActivate would drag the palette
    ; back on screen right after the toggle closed it.
    if wasActive
        return

    ; Opening: FORCE it foreground instead of hoping it wins the race. Sending
    ; the hotkey alone is racy -- CmdPal shows its window and calls
    ; SetForegroundWindow, but another process asserting focus at the same moment
    ; can win, leaving the palette drawn on screen yet not foreground, so every
    ; keystroke goes to the previous window. That is the "I can't type in the
    ; palette" symptom, measured with the window visible at 800x480 while the
    ; foreground window was WezTerm.
    ; AHK just handled this keypress, so it holds foreground rights and its
    ; WinActivate is allowed to reassign focus.
    if WinWait('ahk_exe Microsoft.CmdPal.UI.exe', , 1.5) {
        if !WinActive('ahk_exe Microsoft.CmdPal.UI.exe')
            WinActivate('ahk_exe Microsoft.CmdPal.UI.exe')
    }
}

; ------------------------------------------------------------
; Ctrl+Alt+Space -> cycle the input language.
;
; The OS normally offers Win+Space and Alt+Shift for this. Win+Space is taken
; above (Command Palette), and Alt+Shift is now disabled in the registry because
; it fires by accident far too easily -- a stray Alt+Shift stepped the input
; language on to the Chinese IME and every keystroke after that came out as
; pinyin, with no obvious way back. This is the deliberate replacement.
; ------------------------------------------------------------
^!Space:: PostMessage(0x0050, 2, 0, , 'A')   ; WM_INPUTLANGCHANGEREQUEST, FORWARD

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
