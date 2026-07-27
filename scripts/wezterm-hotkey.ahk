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
    ; Our own launcher. One press opens it, the next closes it -- the launcher
    ; itself toggles on a named event, so holding Super and tapping Space
    ; repeatedly works without AHK tracking any state.
    ;
    ; No chord is synthesised. `--show` signals the resident instance and exits.
    ; Nothing here injects keystrokes, because synthetic Win presses on this
    ; machine desynchronise AltSnap and it starts swallowing the spacebar.
    Run('"' . EnvGet('USERPROFILE') . '\dev\target\release\launcher.exe" --show', , 'Hide')
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
; Ctrl+Alt+Shift, NOT Ctrl+Alt. On a Spanish layout Ctrl+Alt IS AltGr, which is
; pressed constantly for @ # [ ] { } \ -- so Ctrl+Alt+Space was one stray AltGr
; away from silently cycling the input language, and landing on the Japanese or
; Chinese IME turns the spacebar into a conversion key that stops typing spaces
; at all. Requiring Shift as well puts it out of accidental reach.
^!+Space:: PostMessage(0x0050, 2, 0, , 'A')   ; WM_INPUTLANGCHANGEREQUEST, FORWARD

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

; ------------------------------------------------------------
; Win+Shift+Z -> panic key: restart AltSnap and reload this script.
;
; The escape hatch. AHK tracks modifier state itself, and if that ever drifts out
; of step with the real keyboard -- it can, and synthetic input makes it more
; likely -- the script believes Super is held, every bare Space matches #Space
; above, and the hook swallows it before any application sees it. The keyboard
; then looks broken: no spaces, no Win+Space, no Win+Alt+Space reaching GlazeWM.
; Reloading resets that state. Deliberately reachable without the spacebar, and
; on Z because GlazeWM already owns lwin+shift+r for its resize binding mode.
; ------------------------------------------------------------
#+z:: {
    ; AltSnap goes first: its Hotkeys are 5B 5C, i.e. the Windows key, and it
    ; tracks that key's state itself. When that tracking desynchronises it keeps
    ; believing Super is held and swallows ordinary keystrokes -- the spacebar
    ; among them -- while the OS itself reports no modifier at all. Killing it is
    ; the fix; the supervisor puts it back within its next tick.
    ProcessClose('AltSnap.exe')
    Reload
}
