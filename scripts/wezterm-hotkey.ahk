#Requires AutoHotkey v2.0
#SingleInstance Force

; ------------------------------------------------------------
; Latido, para que se pueda SABER si esto esta vivo.
;
; Alt+F10 fallo dos veces sin dejar rastro: ni clip, ni aviso, ni una linea en
; ningun log. Desde fuera, "AHK caido", "el atajo no llega" y "el guardado fallo"
; se ven exactamente igual -- no hay nada. Esto convierte la primera en una
; pregunta con respuesta.
;
; Se escribe desde un temporizador, o sea desde el bucle de mensajes: si el
; script se cuelga o alguien lo suspende, el archivo envejece. La barra lo lee y
; pinta el estado en el boton de guardar.
; ------------------------------------------------------------
global AhkLastF10 := 0

AhkAlive() {
    global AhkLastF10
    f := EnvGet('USERPROFILE') . '\.config\ahk-alive.json'
    ; El latido en si es la FECHA DEL ARCHIVO, no un campo dentro: A_TickCount
    ; se reinicia al arrancar la maquina y no se puede comparar con nada de
    ; fuera. Dentro va solo lo que la fecha no puede decir.
    ;
    ; suspended cuenta como caido a efectos practicos: con los atajos
    ; suspendidos el proceso sigue vivo y respondiendo, y nada mas lo delataria.
    txt := '{"suspended":' . (A_IsSuspended ? 'true' : 'false')
         . ',"lastF10":' . AhkLastF10 . '}'
    try FileDelete(f)
    try FileAppend(txt, f)
}
SetTimer(AhkAlive, 5000)
AhkAlive()

; ------------------------------------------------------------
; Interruptor de los atajos, mandado desde la barra.
;
; La barra escribe un 1 o un 0 en ~/.config/ahk-suspend.flag y esto lo obedece.
; Un ARCHIVO y no una pulsacion sintetica: inyectar teclas desde la barra es lo
; que desincroniza AltSnap y acaba comiendose la barra espaciadora. Esa via esta
; prohibida en este rice.
;
; Se sondea cada 250 ms y no en el latido de 5 s: pulsar el icono y esperar cinco
; segundos a que el color cambie se siente roto.
;
; Suspend(true) NO afecta a este temporizador ni al latido: los temporizadores
; siguen corriendo suspendidos, que es justo lo que hace falta para poder
; volver a encenderlo.
; ------------------------------------------------------------
AhkSuspendFlag() {
    f := EnvGet('USERPROFILE') . '\.config\ahk-suspend.flag'
    quiere := false
    if FileExist(f) {
        try quiere := Trim(FileRead(f)) = '1'
    }
    if (quiere && !A_IsSuspended) {
        Suspend(true)
        AhkAlive()          ; que la barra se entere ya, sin esperar al latido
    } else if (!quiere && A_IsSuspended) {
        Suspend(false)
        AhkAlive()
    }
}
SetTimer(AhkSuspendFlag, 250)

; ------------------------------------------------------------
; Hyprland-style global hotkey to launch WezTerm.
;   #  = SUPER (Windows key)
;   SUPER + Enter  -> open a new WezTerm window, instantly
; Runs from the Startup folder, so the hotkey is always active.
;
; 2026-08-28: back to plain `start`, no `cli spawn` at all. Two earlier
; attempts both used something that persists behind the window closing --
; a custom mux domain first, then `wezterm cli spawn --new-window` -- and
; both bit us for the same underlying reason. The mux domain kept
; reattaching to a stale session. `cli spawn --new-window`, when NO
; wezterm-gui is already running, turned out to silently start its OWN
; wezterm-mux-server.exe as an implicit fallback and then hang on it --
; reproduced directly: a single cold call sat for 80+ seconds and left an
; orphaned mux-server process behind. Nine of those had piled up in the
; background before this was caught, each having blocked RunWait for that
; same long hang -- that IS the "Win+Enter takes forever" symptom.
;
; Plain `start` has none of this: no persistence, no implicit domain, a
; fresh wezterm-gui.exe process every time. Slightly more memory per
; window, but that was never the actual cost on this machine (see
; .wezterm.lua's own comment on this), and it's the one form of this
; hotkey that has never once misbehaved.
; ------------------------------------------------------------

#Enter:: Run('"C:\Program Files\WezTerm\wezterm-gui.exe" start')

; ------------------------------------------------------------
; Stop a BARE Win press from opening the Start menu, without breaking any
; Win+... combination.
;
; Windows opens Start on Win *keyup* only when no other key was pressed in
; between. So: `~` passes the real LWin through (GlazeWM's lwin+... bindings
; still see it), and we immediately send vkE8 -- an
; unassigned virtual key that does nothing -- so the OS sees a combination and
; leaves Start alone.
;
; Remapping LWin outright would have been simpler and wrong: GlazeWM listens for
; the physical LWin, so the entire keybind set would have died with it.
; ------------------------------------------------------------
~LWin::Send '{Blind}{vkE8}'

; ------------------------------------------------------------
; Win+Space -> our launcher, NOT the OS language switch.
;
; Windows fires its input-language switcher on Win+Space even with a low-level
; hook in place, so AHK has to CLAIM the combo -- that is what suppresses the
; switch -- and then act on it itself.
;
; This used to forward to PowerToys' Command Palette on Win+Ctrl+Space. That is
; gone: crates/launcher does the job now and is invoked directly, so there is no
; second hotkey in the middle.
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
; above (the launcher), and Alt+Shift is now disabled in the registry because
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
$!F10:: {
    ; La aplicacion activa se lee AQUI, y se le pasa al script.
    ;
    ; Esto es lo unico que corre en el instante de la pulsacion. El script de
    ; guardado lo intentaba por su cuenta con GetForegroundWindow y siempre veia
    ; otra cosa: lanzar pwsh -- incluso con 'Hide' -- asigna una consola, y esa
    ; ventana ya ha movido el foco cuando el script arranca. El sintoma era que
    ; ningun clip de League llegaba a Discord; el log decia "no era League" con
    ; la partida delante.
    ;
    ; De paso deja de importar cuanto tarde el script en arrancar.
    ; Se anota que ESTA pulsacion llego. Con esto, "no guardo nada" deja de ser
    ; ambiguo: si el sello avanza, la tecla llego y el fallo es de aqui abajo.
    ;
    ; Segundos desde epoch, no A_TickCount: tiene que sobrevivir a un reinicio y
    ; poder compararse desde otro proceso.
    global AhkLastF10 := DateDiff(A_NowUTC, '19700101000000', 'Seconds')
    AhkAlive()

    proc := ""
    try proc := WinGetProcessName("A")

    ; El corte se pide AQUI, antes de lanzar nada.
    ;
    ; El grabador cierra el segmento en el fotograma siguiente y vuelca el anillo
    ; a disco; eso son ~300 ms que hasta ahora empezaban a contar DESPUES de que
    ; arrancara pwsh (~250 ms). Pidiendolo desde aqui, las dos cosas corren a la
    ; vez y el segmento suele estar listo antes de que el script pregunte.
    ;
    ; 0x0002 es EVENT_MODIFY_STATE, lo justo para SetEvent. Si el grabador no
    ; esta, OpenEventW devuelve 0 y no pasa nada: el script lo intenta por su
    ; cuenta como antes.
    ; El sello va ANTES de señalizar, y no es opcional.
    ;
    ; El script necesita distinguir "este corte es el mío" de "este corte es el
    ; de la pulsación anterior". Antes lo hacía por la EDAD de la marca (menos de
    ; 2 s = mía), y eso falla con dos Alt+F10 seguidos: el segundo se quedaba con
    ; el corte del primero y guardaba un clip que terminaba hasta 2 s antes de la
    ; segunda pulsación. Con el sello la pregunta es exacta: ¿se cerró el
    ; segmento DESPUÉS de que yo lo pidiera?
    stamp := EnvGet('USERPROFILE') . '\ShadowPlay\wgc-buffer\cut-requested.txt'
    try FileDelete(stamp)
    try FileAppend('1', stamp)

    h := DllCall("OpenEventW", "UInt", 0x0002, "Int", 0, "Str", "Global\rice-shadowplay-cut", "Ptr")
    if h {
        DllCall("SetEvent", "Ptr", h)
        DllCall("CloseHandle", "Ptr", h)
    }

    Run('pwsh -NoProfile -WindowStyle Hidden -File "' . EnvGet('USERPROFILE') . '\.config\shadowplay-wgc-save.ps1" -Foreground "' . proc . '"', , 'Hide')
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
