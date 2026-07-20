# 🦆 rice — Windows tiling desktop

Un escritorio Windows 11 tipo Hyprland, hecho desde cero: **WezTerm + fastfetch**,
**GlazeWM** en tiling fibonacci, una **barra de estado nativa en Rust** (~6 MB), y un
**ShadowPlay propio** (AV1 + audio del sistema). Todo con la tecla **SUPER** y
optimizado para pesar lo mínimo (~340 MB todo el stack).

---

## Instalación

Requiere **Windows 11** y **PowerShell 7** (`pwsh`). Un paso pide admin (UAC) para
desactivar servicios; lo demás corre como usuario normal.

```powershell
git clone https://github.com/ZBishopM/windots $HOME\dotfiles
cd $HOME\dotfiles
pwsh -ExecutionPolicy Bypass -File .\install.ps1
```

El `install.ps1`:

1. Instala dependencias con **scoop** (`fastfetch glazewm altsnap autohotkey ffmpeg`) +
   **wezterm** y **rust** con winget.
2. Copia el proyecto Rust a `~/dev/glaze-bar` y lo **compila** (`cargo build --release`).
3. Despliega los configs a su sitio, **reescribiendo la ruta del home** a la tuya.
4. Crea las carpetas de ShadowPlay, los accesos de **autostart**, y aplica los tweaks
   de registro/env.

Cierra sesión y vuelve a entrar (o reinicia) para que arranque todo.

---

## Componentes

| Pieza | Qué es | Dónde |
|---|---|---|
| **WezTerm + fastfetch** | Terminal con pato ASCII + specs al abrir | `~/.wezterm.lua`, `~/.config/fastfetch/` |
| **GlazeWM** | Tiling window manager (binds SUPER) | `~/.glzr/glazewm/config.yaml` |
| **dwindle** | Layout fibonacci vía IPC de GlazeWM | `~/.config/glazewm-dwindle.ps1` |
| **glaze-bar** | Barra de estado nativa (Rust/egui), 1 por monitor | `~/dev/glaze-bar` |
| **AltSnap** | Mover/redimensionar con SUPER+arrastrar | `~/scoop/.../AltSnap.ini` |
| **ShadowPlay** | Buffer rodante de 30 s (ffmpeg AV1 + audio) | `~/.config/shadowplay-*` |
| **shadowplay-notify** | Toast animado al guardar un clip (Rust) | binario en `~/dev/glaze-bar` |
| **sysaudio-loopback** | Captura audio del sistema (WASAPI, Rust) | binario en `~/dev/glaze-bar` |
| **rice-supervisor** | Watchdog: revive cualquier componente que muera (<60s) | `~/.config/rice-supervisor.ps1` |
| **cava** | Visualizador de espectro de audio en terminal (FFT, 165fps) | binario en `~/dev/glaze-bar`, comando `cava` |

Los **3 binarios Rust** viven en un solo proyecto cargo: `glaze-bar`, `shadowplay-notify`,
`sysaudio-loopback`.

---

## Atajos (todo en SUPER = tecla Windows)

| Acción | Atajo |
|---|---|
| Abrir WezTerm | `SUPER + Enter` |
| Command Palette (buscador) | `SUPER + Space` |
| Enfocar ventana | `SUPER + ← ↑ ↓ →` (o `H K J`) |
| Mover ventana | `SUPER + Shift + ←↑↓→` |
| Redimensionar | `SUPER + U I O P` |
| Flotar ⇄ tile | `SUPER + Alt + Space` |
| Fullscreen | `SUPER + F` |
| Cerrar ventana | `SUPER + Q` |
| Ir a workspace 1–9 | `SUPER + 1…9` |
| Enviar a workspace | `SUPER + Shift + 1…9` |
| Mover / resize / swap (mouse) | `SUPER + arrastrar` |
| Modo resize (hjkl) | `Alt + Shift + R` |
| **Guardar replay 30 s** | `Alt + F10` → clip en `~/ShadowPlay/clips` |

**Excepciones** (conflictos de Windows): foco-derecha solo con flecha (`SUPER+L` bloquea
la pantalla y ningún hook lo intercepta); cycle-focus en `SUPER+Shift+Space` porque
`SUPER+Space` es el buscador.

---

## Pasos manuales

- **Command Palette en Win+Space**: instala **PowerToys**, activa *Command Palette*.
  Windows dispara el cambio de idioma en `Win+Space` aunque el *low-level hotkey* esté
  activo, así que: pon el atajo de CmdPal en **`Win+Ctrl+Space`**, y el `wezterm-hotkey.ahk`
  reenvía `Win+Space` → `Win+Ctrl+Space` (bloqueando el cambio de idioma). Alternativa:
  cualquier launcher.
- **Layout de monitores**: las posiciones están **hardcodeadas a 1920 (principal) + 2560
  (secundario)**. Ajusta a tus pantallas:
  - `~/.glzr/glazewm/config.yaml` → `startup_commands`: los `--x`/`--width` de las 2 barras.
  - `shadowplay-notify.rs` → `with_position` (esquina de la notificación).
  - `main.rs` de glaze-bar si cambias resoluciones raras.
- **Servicios** (si saltaste el paso admin): en PowerShell elevado
  `Set-Service DiagTrack,SysMain,DPS,Spooler -StartupType Disabled` y `Stop-Service ...`.

---

## Personalización rápida

| Quiero… | Editar |
|---|---|
| Otro monitor a grabar | `shadowplay-record.ps1` → `output_idx=0` → `1` |
| Menos RAM al grabar | `shadowplay-record.ps1` → `-preset p6` → `p4`, o `-cq 19` → `23` |
| Balancear juego/mic | `shadowplay-record.ps1` → `amix ... normalize=0` → pesos |
| Micrófono preferido | `shadowplay-record.ps1` → `$prefer = @('Blue Snowball','HyperX')` |
| Duración del replay | `shadowplay-save.ps1` → `Select-Object -Last 6` (6 × 5 s = 30 s) |
| Módulos de la barra | `glaze-bar/src/main.rs`, luego `cargo build --release` |

Tras editar un binario Rust: `cd ~/dev/glaze-bar; cargo build --release` y reinicia GlazeWM
(`Alt+Shift+E`) o el proceso.

---

## Notas

- **komorebi** no funciona en este equipo (`os error 1920`, bloqueo de seguridad del
  binario). Por eso GlazeWM.
- El **grabador** (~240 MB) es el grueso de la RAM — precio de grabar siempre. El WM solo
  pesa ~100 MB.
- `sysaudio-loopback` captura el **default render endpoint** (lo que suene), sin drivers
  de terceros ni cambiar tu routing.
- **MPO desactivado** (`OverlayTestMode=5`): los overlays de hardware para video (MPO)
  saltan la composición de DWM, así que Desktop Duplication (ddagrab) **no los ve** y el
  ShadowPlay graba un **frame congelado** cuando reproduces video acelerado. El `install.ps1`
  lo apaga (paso admin); **requiere reboot**. Revertir: `reg delete "HKLM\SOFTWARE\Microsoft\Windows\Dwm" /v OverlayTestMode /f`.

## Estructura del repo

```
dotfiles/
├─ install.ps1              # aplica todo
├─ wezterm/.wezterm.lua
├─ config/fastfetch/        # config.jsonc + duck.txt
├─ config/glazewm/config.yaml
├─ powershell/…profile.ps1
├─ scripts/                 # dwindle, wezterm-hotkey, shadowplay-*
├─ altsnap/AltSnap.ini
└─ glaze-bar/               # proyecto cargo (3 binarios)
   ├─ Cargo.toml · Cargo.lock
   └─ src/{main.rs, bin/shadowplay-notify.rs, bin/sysaudio-loopback.rs}
```
