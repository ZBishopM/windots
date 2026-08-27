# 🦆 rice — Windows tiling desktop

Un escritorio Windows 11 tipo Hyprland, hecho desde cero: **WezTerm + fastfetch**,
**GlazeWM** en tiling fibonacci, una **barra de estado nativa en Rust** (~6 MB), y un
**ShadowPlay propio** (AV1 + audio del sistema). Todo con la tecla **SUPER** y
optimizado para pesar lo mínimo: **~520 MB todo el stack**, medido, y de esos
**178 MB son el grabador** — con el buffer rodante de 30 s corriendo — más otros
125 MB del índice de archivos del buscador. Los dos hosts de PowerShell
(supervisor y dwindle) suman **26 MB**.

Las cifras por proceso oscilan bastante y conviene saber por qué antes de
perseguir un pico: varios componentes llaman a `EmptyWorkingSet` cuando están
ociosos, así que sus páginas van a la lista de espera y vuelven al primer uso.
dwindle mide 15 MB o 5 MB según el momento en que mires, y el buscador 125 MB
recién indexado o 5 MB después. Una sola foto no dice nada; el número de arriba
es con todo caliente.

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
2. Copia los crates Rust a `~/dev` y compila el **workspace** (`cargo build --release`) → binarios en `~/dev/target/release/`.
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
| **nushell** | Shell interactivo por defecto (~67 ms a prompt vs ~350 ms de pwsh). Mismo fastfetch, tema cálido del rice, completado difuso con menús, y badges de contexto (node/pnpm · python · rust · rama git · ssh) calculados sólo al cambiar de directorio | `~/AppData/Roaming/nushell/config.nu` |
| **PowerShell 7** | Sigue ejecutando toda la infraestructura (`.ps1`), invocada como `pwsh -File`. Para un prompt de pwsh: escribe `pwsh`, o elígelo en el menú de lanzamiento de WezTerm | `~/Documents/PowerShell/…profile.ps1` |
| **dwindle** | Layout fibonacci vía IPC de GlazeWM | `~/.config/glazewm-dwindle.ps1` |
| **glaze-bar** | Barra de estado nativa (Rust/egui), 1 por monitor | `~/dev/glaze-bar` |
| **AltSnap** | Mover/redimensionar con SUPER+arrastrar | `~/scoop/.../AltSnap.ini` |
| **ShadowPlay (WGC)** | Buffer rodante de 30 s vía Windows.Graphics.Capture (HEVC hardware, ~38% menos GPU que ddagrab) + audio del sistema | `~/dev/shadowplay-wgc`, `~/.config/shadowplay-wgc-*` |
| **ShadowPlay (ddagrab)** | Grabador anterior (ffmpeg AV1 + loopback). Fallback del tag v1.0 | `~/.config/shadowplay-record.*` |
| **shadowplay-notify** | Toast animado al guardar un clip (Rust) | binario en `~/dev/target/release/` |
| **sysaudio-loopback** | Captura audio del sistema o del micro (WASAPI, Rust) | binario en `~/dev/target/release/` |
| **micswitch** | Alterna el micro por defecto entre los de `rice.json` | binario en `~/dev/target/release/`, comando `mic` |
| **ws-slide** | Animación de deslizamiento al cambiar de workspace. **Es dueño de SUPER+1..9** (GlazeWM ya no los bindea), así que si muere, el cambio de workspace deja de funcionar hasta que el supervisor lo reviva | binario en `~/dev/target/release/` |
| **taskbar** | Oculta la barra de tareas de Windows **y lee su bandeja** para que glaze-bar la pinte (ver abajo) | `~/dev/crates/taskbar` |
| **rice-supervisor** | Watchdog: revive cualquier componente que muera (<60s). Tabla de componentes + log en `~/.config/logs/` | `~/.config/rice-supervisor.ps1` |

#### La bandeja del sistema en nuestra barra

Los iconos de la bandeja (Discord, Tailscale, G HUB…) salen a la derecha de glaze-bar,
con su tooltip al pasar por encima y clic izquierdo para activarlos. Cómo, y por qué así:

- **No se puede alojar la bandeja.** `Shell_NotifyIcon` manda un `WM_COPYDATA` a la
  ventana `Shell_TrayWnd`, y sólo puede haber una: la de explorer. Recibir los iconos
  nosotros sería sustituir el shell entero.
- **El toolbar clásico ya no existe.** El método de siempre —
  `Shell_TrayWnd > TrayNotifyWnd > SysPager > ToolbarWindow32` y `TB_GETBUTTON`— está
  muerto: medido en Windows 11 26200, **`SysPager` y `ToolbarWindow32` devuelven 0**.
  La bandeja es XAML.
- **Queda UI Automation**, con una pega: con la barra de tareas en `SW_HIDE` el árbol
  XAML no se realiza y UIA **no ve ni un hijo**. Necesita estar visible.
- **Solución: visible pero invisible.** `taskbar` la deja mostrada —para que el XAML
  exista— con `WS_EX_LAYERED` a alfa 0 (no pinta un píxel) y `WS_EX_TRANSPARENT` (no se
  come clics). El `ABM_SETSTATE` de auto-ocultar se mantiene, así que el área de trabajo
  sigue siendo la pantalla entera y GlazeWM no pierde la franja. Si `taskbar` muere, los
  estilos se quedan puestos: el modo de fallo es que siga sin verse.
- **Los píxeles** salen de `PrintWindow(PW_RENDERFULLCONTENT)` sobre la propia barra,
  recortando cada icono. Sin ese flag la captura sale **entera en negro**. El fondo
  acrílico se descuenta midiendo las esquinas del recorte.

**Iconos ocultos:** Windows guarda la mayoría detrás de *Mostrar iconos ocultos*, y ahí
UIA no los ve hasta abrir el desplegable. Como la barra de tareas es invisible no puedes
sacarlos arrastrando, así que:

```powershell
pwsh -File ~/.config/rice-tray-promote.ps1          # promociona lo que esté corriendo
pwsh -File ~/.config/rice-tray-promote.ps1 -All     # todo lo que exista en disco
pwsh -File ~/.config/rice-tray-promote.ps1 -Reset   # todo de vuelta al desplegable
```
| **cava** | Visualizador de espectro de audio en terminal (FFT, 165fps) | binario en `~/dev/target/release/`, comando `cava` |
| **launcher** | El buscador de `Win+Space`. Busca **aplicaciones**, **archivos** (índice propio, ver abajo) y **comandos**, difuso y con iconos. `Ctrl+Enter` abre como administrador | `~/dev/crates/launcher` |

**Comandos** son tres cosas distintas en la misma lista:
- **Acciones del sistema** que no tienen acceso directo: bloquear, suspender, apagar, cerrar
  sesión, administrador de tareas/dispositivos, servicios, `ms-settings:*`, papelera, y dos
  del rice (`Editar rice.json`, `Recargar la configuración de GlazeWM`). Se buscan en
  español o en inglés (`lock` encuentra *Bloquear la sesión*).
- **Ejecutar** cualquier cosa del `PATH`, con argumentos — el reemplazo de `Win+R`. El PATH
  se recorre **una vez al arrancar**; resolver a mano serían ~120 consultas al disco por
  tecla pulsada.
- **Rutas y URLs**: una ruta que existe se abre, y `www.…` o `http…` van al navegador.
  Prefijo `>` fuerza la ejecución sin comprobar nada.

#### Por qué el índice de archivos es propio

Las tres opciones se midieron en esta máquina antes de elegir; el razonamiento largo
está en la cabecera de `crates/launcher/src/files.rs`.

| Opción | Por qué no / por qué sí |
|---|---|
| **Índice de Windows** (WSearch, `Search.CollatorDSO`) | Descartado. Tiene ~196k elementos de los ~2,17M que hay: excluye `AppData`, todos los dotfolders y **D: entero**. Cuesta 218 MB residentes. Y **no tiene infijos** — `LIKE '%x%'` son 133-206 ms frente a 4-9 ms de un prefijo — así que un buscador difuso no puede montarse encima. Además devuelve rutas localizadas (`C:\Usuarios\…`) que no existen en disco. |
| **Everything por IPC** | Descartado por dependencia, no por técnica: el IPC es fácil (`WM_COPYDATA` sobre `EVERYTHING_TASKBAR_NOTIFICATION`, o el crate `everything-ipc`). Pero leer la MFT **exige administrador**, por eso instala un servicio; sin él degrada a escanear directorios, que es justo lo que hacemos nosotros. |
| **Propio** ✅ | Recorrido paralelo en memoria, sin admin ni servicio ni terceros. **1.815.240 entradas en 6,0 s**, **68 MB**, búsquedas de **8,6-31,8 ms**, 0,00% de CPU en reposo. Se mantiene al día con `ReadDirectoryChangesW`. Cobertura ajustable en `launcher.file_roots` / `file_skip`: el `rice.json` de aquí excluye `:\windows` y `:\program files` — anclados con dos puntos a propósito, porque un `\windows` suelto también excluiría la carpeta `windows/` de cada proyecto Flutter o CMake. |

Como esto cubre el 100% del disco y WSearch el 12%, se puede apagar el servicio
`WSearch` y recuperar sus 218 MB — a cambio de perder la búsqueda del menú Inicio y
la del Explorador, que siguen dependiendo de él.

### Configuración y sincronización

| Archivo | Para qué |
|---|---|
| `~/.config/rice.json` | **Toda la configuración.** Ver [la tabla completa](#ricejson-todas-las-claves) |
| `~/.config/rice.schema.json` | Esquema JSON del anterior: tu editor autocompleta y valida. Cada clave lleva su `description` |
| `~/.config/lib/*.ps1` | Librería común de los scripts: rutas (`rice-paths`), cliente IPC de GlazeWM (`rice-ipc`), helpers de procesos (`rice-proc`) |
| `~/.config/rice-secrets.json` | **No versionado.** Webhook de Discord y userhash de catbox. Este repo es público |
| `sync.ps1` | Copia el sistema vivo → repo (lo inverso de `install.ps1`). `-Check` sólo reporta diferencias y sale con código 1 |

#### Dónde está el código de verdad

**`~/dev/crates` es el árbol vivo. `dotfiles/crates` es una copia** que mueve
`sync.ps1`. Se edita y se compila en `~/dev`; el repo se actualiza después.
Editar dentro de `dotfiles/` y compilar allí no cambia nada de lo que corre.

```
editar ~/dev/crates/...  →  cd ~/dev; cargo build --release  →  reiniciar el
proceso  →  pwsh dotfiles/sync.ps1   (trae el cambio al repo)
```

Los binarios viven en un **workspace cargo** (`~/dev/Cargo.toml`, **13 crates**):
`rice-common` (la librería compartida, no produce ejecutable) más doce binarios que
compilan juntos a `~/dev/target/release/`. Al estar en el mismo directorio, el
grabador y `cava` encuentran a su hermano `sysaudio-loopback.exe` sin paso de copia.

---

### rice.json: todas las claves

La columna que importa es la última. **En caliente** significa que guardas el
archivo y el cambio se ve en ≤2 s sin reiniciar nada — lo hace
`Settings::live()` (`rice-common/src/settings.rs`), que mira la fecha del archivo
como mucho una vez por segundo y sólo relee si cambió. **Reinicio** significa que
el valor se lee una vez con `Settings::get()` (un `OnceLock` que no revalida
nunca) y hay que reiniciar el proceso que lo usa.

| Clave | Tipo | Por defecto | Cuándo se aplica |
|---|---|---|---|
| `ipc_url` | string | `ws://127.0.0.1:6123` | Reinicio. **Nunca `localhost`**: resuelve a `::1` primero y cada conexión pierde ~2,1 s esperando el timeout de IPv6 |
| `bar_height` | 16–120 | `34` | **Reinicio de la barra.** Lo usan también `ws-slide` (filas que la animación no toca) y el recorte de la ventana |
| `mics` | string[] | `["hyperx","snowball"]` | Reinicio. Orden de preferencia de `micswitch` |
| `outputs` | string[] | `["hyperx","vg270","airpods"]` | En caliente, al reabrir el panel de dispositivos |
| `clickthrough_apps` | string[] | `[]` | Reinicio. Aplicaciones sobre las que la barra deja pasar los clics aunque la geometría no diga "pantalla completa" |
| `hide_bar_on_fullscreen` | bool | `true` | **En caliente.** `false` = la barra se sigue dibujando sobre una aplicación a pantalla completa (los clics la atraviesan igual) |
| `notification_style` | `toast` \| `island` \| `both` | `toast` | En caliente. Sobre un juego a pantalla completa usa SIEMPRE la isla, diga lo que diga |
| `launcher.index_store_apps` | bool | `false` | Reinicio del launcher |
| `launcher.index_files` | bool | `true` | Reinicio del launcher |
| `launcher.file_skip` | string[] | 23 rutas | Reinicio. Subcadenas en minúsculas; una ruta que contenga cualquiera no se indexa |
| `launcher.file_roots` | string[] | `[]` | Reinicio. Vacío = todas las unidades fijas |
| `launcher.file_limit` | int | `4000000` | Reinicio. Tope de entradas del índice |
| `clips.*` | — | — | Por guardado. Lo leen los **scripts**, no los binarios: ver [Compartir clips](#compartir-clips-catbox--discord) |
| `animation.*` | — | — | **En caliente**, ver abajo |

---

### Compartir clips: catbox + Discord

No es un bot: es un **webhook**. Publica en un canal y nada más — no lee, no
responde, no tiene comandos.

#### Qué pasa al pulsar Alt+F10

```
Alt+F10
  └─ shadowplay-wgc-save.ps1        corta ~30 s del búfer en RAM
     │                              y mira QUÉ APP TENÍA EL FOCO en ese instante
     └─ rice-clip-share.ps1         (en segundo plano: tarda ~15 s)
        ├─ transcodifica HEVC → H.264
        ├─ sube a catbox (o litterbox)
        ├─ enlace al portapapeles          ← inmediato
        ├─ publica el enlace en Discord    ← inmediato, sólo si el foco era un juego
        └─ aviso en tu pantalla            ← APLAZADO hasta que sueltes el juego
```

Tres decisiones que explican el diseño:

**Se transcodifica** porque Discord es Chromium y no decodifica H.265. Un clip
subido tal cual le llega a tus amigos como una descarga en vez de reproducirse
en el canal. Cuesta ~6 s con NVENC y **no** ahorra tamaño: medido aquí, un clip
bajó de 25,9 a 13,8 MB y otro **subió** de 24,5 a 27,3. Si el H.264 engorda más
de la mitad, se descarta y se sube el original.

**Se publica el enlace, no el archivo.** El webhook usa el nivel de *boost* del
servidor (10 MB sin boost, 50 en nivel 2, 100 en nivel 3) y un clip medio son
~26 MB. Un enlace cabe siempre, y Discord previsualiza el `.mp4` en el canal.

**A Discord se publica YA; el aviso en tu pantalla espera.** Publicar no te
interrumpe —aparece en la pantalla de otros— y el sentido de compartir una
jugada es que la vean mientras sigues jugando. Lo que se aplaza es la
notificación local, que expande la barra y minimizaba el juego en pantalla
completa. Tope de 15 min: pasado eso avisa igual.

#### Dónde se configura

| Archivo | Qué lleva | Versionado |
|---|---|---|
| `~/.config/rice.json` → `clips` | Si sube, a dónde, si transcodifica, en qué juegos avisa | Sí |
| `~/.config/rice-secrets.json` | **La URL del webhook y el userhash de catbox** | **No** |
| `~/.config/logs/clip-share.log` | Qué pasó en cada guardado, con el código HTTP | No |

Las claves de `clips.*` están documentadas una a una en `rice.schema.json`; tu
editor las autocompleta. Resumen: `upload`, `host` (`catbox`\|`litterbox`),
`litterbox_time`, `transcode`, `cq`, y `discord_when_focused`.

`discord_when_focused` es una lista de subcadenas en minúsculas comparadas
contra el **nombre del proceso enfocado** al pulsar Alt+F10. `League of Legends`
es el juego; `LeagueClientUx` es el lanzador y **no** hace match a propósito,
porque un clip del lobby no es una jugada.

#### rice-secrets.json

**Fuera del repo a propósito**: con la URL del webhook cualquiera puede publicar
en tu canal, y este repo es público. Está en `.gitignore`, no está en el mapa de
`sync.ps1`, y `install.ps1` sólo crea una plantilla vacía.

```jsonc
{
  "discord_webhook": "https://discord.com/api/webhooks/<id>/<token>",
  "catbox_userhash": "<32 caracteres>"
}
```

| Clave | Dónde se saca | Si falta |
|---|---|---|
| `discord_webhook` | Discord → Ajustes del servidor → Integraciones → Webhooks → Nuevo webhook → Copiar URL | Sube igual y deja el enlace en el portapapeles; el log dice `era League pero no hay discord_webhook` |
| `catbox_userhash` | catbox.moe → Manage account → User hash | La subida es **anónima y NO SE PUEDE BORRAR**: la API de borrado exige userhash y responde 412 |

Ese último punto es el motivo de que **`clips.upload` venga en `false` por
defecto**: si el archivo de configuración falta o no se puede leer, el fallo
seguro es no publicar nada.

#### Cuando no llega a Discord

Mira `~/.config/logs/clip-share.log`. Ya no hay salidas mudas: si el script
corrió, dejó una línea. Las tres que importan:

| Línea | Significa |
|---|---|
| `no era League: no aviso a Discord` | El foco al pulsar Alt+F10 no coincidía con `discord_when_focused` |
| `era League pero no hay discord_webhook en rice-secrets.json` | Falta el secreto |
| `Discord fallo: …` | El webhook existe pero la petición falló — webhook borrado, canal borrado, o sin red |

---

### Cambiar la barra

#### Animación — sin recompilar

Edita `~/.config/rice.json` y guarda. El efecto tarda ≤2 s: hasta 1 s del sondeo
por fecha y hasta 1 s del repintado en reposo. Si la barra está animando algo
(espectro sonando, panel abierto) es inmediato.

| Clave | Por defecto | Qué mueve |
|---|---|---|
| `pill_ease` | `15.0` | Velocidad con la que la píldora central crece y encoge (ancho **y** alto). Más alto = más seco |
| `spring_stiffness` | `300.0` | Rigidez del muelle vertical del panel desplegable |
| `spring_damping` | `23.0` | Amortiguación de ese muelle. **Deliberadamente subamortiguado**: rebota y se asienta, que es lo que lo hace parecer una burbuja saliendo de la barra y no un menú apareciendo. Subirlo lo vuelve sobrio; bajarlo, gomoso |
| `workspace_ease` | `16.0` | Deslizamiento del indicador ámbar entre workspaces |
| `text_ease` | `14.0` | Reloj creciendo y desplazándose al centro cuando se abre el panel |
| `notification_hold_secs` | `4.0` | Segundos que una notificación se queda en la isla |
| `panel_timeout_secs` | `6.0` | Inactividad tras la que el panel se cierra solo |
| `spectrum_fps` | `30` | Refresco del visualizador. Es lo único de esta tabla con coste de CPU real |

#### Espaciado y geometría — recompilando

Están **cableados en el código a propósito**: son decisiones de diseño, no
preferencias, y sacarlos a `rice.json` daría una superficie de configuración que
nadie ajusta dos veces. Todos en `crates/glaze-bar/src/main.rs`.

| Qué | Dónde | Ojo |
|---|---|---|
| **Separación de los bordes de pantalla** | `const BAR_PAD_H` (22.0) | Cuánto se apartan del canto los workspaces y los stats/bandeja. Era 10 y quedaba pegado, difícil de leer y de acertar |
| **Aire entre la barra y las ventanas** | `outer_gap.top` en `config/glazewm/config.yaml` (8px) | **Acoplado a la reserva**: la barra registra sus `bar_height` píxeles como *appbar*, así que el área de trabajo ya empieza bajo ella y aquí sólo queda el aire. Si cambias `bar_height`, esto NO hay que tocarlo; si quitaras la reserva, habría que volver a sumar la altura |
| Margen arriba/abajo | `const BAR_PAD_V` (5.0) | Compromiso con el tamaño de la fuente; el alto de la barra es fijo |
| Hueco entre workspaces | `ui.add_space(5.0)` tras cada pill | |
| Padding del pill de workspace | `Margin::symmetric(9.0, 2.0)` | Es lo que hace los números anchos o estrechos |
| Hueco entre métricas | seis `ui.add_space(12.0)` seguidos en el bloque `right_to_left` | Están los seis juntos; cámbialos a la vez |
| Hueco entre iconos de bandeja | `vec2(side + 6.0, side)` en `tray.rs` | |
| Padding de la píldora | `let pad = 14.0;` | |
| Hueco reloj → notificación | `let div_gap = 22.0;` | |
| Alto de la píldora | `if has_extra { 30.0 } else { 24.0 }` | 24 en reposo, 30 con contenido |
| Separación isla ↔ stats | `sl - 12.0 - left_anchor` | La isla **no crece por encima de los stats**; se detiene aquí y el texto lleva puntos suspensivos |
| Radios | `Rounding::same(5.0)` en workspaces, y el `h/2.0 → 20.0` de la píldora | |
| Tamaño de cada panel | `panel_width()` y `panel_target_h()` | Una rama por vista |

Búscalos por el literal, no por número de línea: el archivo se mueve.

Después: `cd ~/dev; cargo build --release`, reinicia la barra, y `sync.ps1`.

> **Reiniciar la barra**: mátala y el supervisor la relanza en ≤30 s. O
> `Get-Process glaze-bar | Stop-Process -Force`.
>
> `GLAZEBAR_PANEL=notifs` abre ese panel al arrancar, para mirar cómo queda sin
> tener que ir clicando. Una instancia de prueba con la misma `--x` que una barra
> real se suicida contra el mutex de instancia única: usa otra `--x`.

---

## Atajos (todo en SUPER = tecla Windows)

| Acción | Atajo |
|---|---|
| Abrir WezTerm | `SUPER + Enter` |
| Buscador (apps, archivos, comandos) | `SUPER + Space` |
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

- **Buscador en Win+Space**: no hay paso manual. Es `crates/launcher`, lo arranca el
  supervisor y `wezterm-hotkey.ahk` le manda `--show`. Sustituye a la *Command Palette*
  de PowerToys, que costaba **267 MB residentes y 59 s de arranque en frío** — medido,
  era la partida más grande del post-inicio. Ojo con `Win+Space`: Windows también lo usa
  para cambiar de idioma, así que el cambio de idioma se movió a `Ctrl+Alt+Shift+Space`
  (`Ctrl+Alt+Space` no vale: en teclado español es `AltGr+Space`).
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
| Duración del replay | `shadowplay-wgc-save.ps1` → `$OBJETIVO = 30` (segundos). La selección va por tiempo acumulado, no por número de segmentos |
| Cuánto historial guarda el búfer | `shadowplay-wgc/src/main.rs` → `RING = 12` (× 5 s ≈ 60 s). **Duplicado**: hay que cambiar también `$RING` en `shadowplay-wgc-save.ps1` |
| Animación de la barra | `~/.config/rice.json` → bloque `animation`, en caliente. [Tabla](#animación--sin-recompilar) |
| Espaciado de la barra | `glaze-bar/src/main.rs`. [Mapa de qué toca qué](#espaciado-y-geometría--recompilando) |

Tras editar un binario Rust: `cd ~/dev; cargo build --release` y reinicia el
proceso (o GlazeWM con `Alt+Shift+E` si es suyo).

---

## Cuando algo no responde

Casi todo el rice es residente, así que "no pasó nada" es el síntoma habitual y
no dice nada por sí solo. Estos archivos existen para que sí lo diga. Todos en
`~/.config/` salvo donde se indique.

| Archivo | Lo escribe | Qué significa |
|---|---|---|
| `ahk-alive.json` | AutoHotkey, cada 5 s | **Su FECHA es el latido**, no un campo de dentro. Rancio = los atajos globales están caídos. Dentro: `suspended` y `lastF10` (cuándo disparó Alt+F10 de verdad) |
| `bar-alive-<x>.txt` | cada barra, desde su bucle de dibujo | Rancio >30 s = la barra está viva pero **dejó de pintar**. El supervisor la reinicia sola |
| `ShadowPlay/wgc-buffer/cut-requested.txt` | AHK al pulsar Alt+F10 | Si no existe o es viejo tras pulsar, **el atajo no llegó a ejecutarse**: el fallo está por encima del script de guardado |
| `ShadowPlay/wgc-buffer/last-finished.txt` | el grabador, sólo al cortar | Qué segmento acaba de cerrarse. No lo tocan las rotaciones normales |
| `notifications.json` | notifyd y `Set-RiceIsland` | Historial del centro de notificaciones. **Si una notificación del sistema se fue y no supiste qué era, está aquí** |
| `logs/glaze-bar.log` | la barra | Cada cambio de click-through, con quién lo provocó |
| `logs/supervisor.log` | el supervisor | Qué relanzó y por qué |
| `logs/clip-share.log` | el guardado de clips | Selección, espera, y el resultado de la subida con su código HTTP |

**El indicador de Alt+F10** está en la fila de stats de la barra: una cámara
verde (activo), ámbar (atajos suspendidos → `Win+Shift+Z`) o roja (AutoHotkey no
responde). Pasa el ratón por encima para el detalle y el último uso.

Orden de comprobación cuando **Alt+F10 no hace nada**:

1. La cámara de la barra: ¿de qué color está?
2. `cut-requested.txt`: ¿es de hace un momento? Si no, la tecla no llegó.
3. `logs/clip-share.log`: ya no hay salidas mudas — si el script corrió, dejó una
   línea, y si no había nada que guardar sale un aviso en la isla.

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
├─ powershell/…profile.ps1  # perfil de pwsh (infra)
├─ nushell/config.nu        # shell interactivo por defecto
├─ scripts/                 # dwindle, wezterm-hotkey, shadowplay-*, supervisor
│  └─ lib/                  # rice-paths · rice-ipc · rice-proc (dot-sourced)
├─ altsnap/AltSnap.ini
├─ sync.ps1                 # live -> repo (inverso de install.ps1)
├─ Cargo.toml · Cargo.lock  # raíz del workspace
└─ crates/                  # un crate por herramienta
   ├─ rice-common/          # lib compartida: theme · ui · win · ipc · config · event
   │                        #   args · settings · audio · brightness · media · spectrum
   ├─ glaze-bar/            # barra de estado (egui)
   ├─ shadowplay-notify/    # toast
   ├─ shadowplay-wgc/       # grabador WGC
   ├─ ws-slide/             # animación de workspace (dueño de SUPER+1..9)
   ├─ sysaudio-loopback/    # captura WASAPI
   ├─ micswitch/            # cambio de micro / salida de audio
   ├─ appvol/               # volumen master y por aplicación
   ├─ taskbar/              # ocultar la barra de tareas + bandeja del sistema
   ├─ launcher/             # el buscador de Win+Space (apps · archivos · comandos)
   ├─ notifyd/              # redibuja TODAS las notificaciones de Windows
   ├─ winkill/              # matar el proceso de la ventana enfocada (SUPER+Shift+Q)
   └─ cava/                 # visualizador de espectro
```

Cada herramienta es su propio crate: así cada una declara sólo lo que usa y
recibe su propio `opt-level` (el workspace compila a 3 por ser casi todo código
en tiempo real, y sólo el toast usa `"z"`). Tocar un binario ya no relinkea el
resto.

---

## Estado, pendientes y qué falta verificar

### Verificado y funcionando

| Pieza | Cómo se comprobó |
|---|---|
| Animación de workspace (`ws-slide`) | Cambio 1→3→1 con las teclas reales; carrusel sin flash |
| Brillo DDC/CI | Ida y vuelta real: 12% → 73% → 12% en el monitor 1, sin tocar el 2 |
| Volumen por app | Discord solo a 30% y de vuelta; vesktop mute/unmute |
| Cambio de salida | Auriculares → monitor → auriculares |
| Barra de tareas | Oculta con área de trabajo de 1080 completos, y restaurada a 1032 |
| Tecla Windows | Inicio bloqueado 3/3, con `Super+N` y `Win+Space` intactos |
| Espectro | Silencio en reposo; barras y `active=true` con audio real |
| Guardado de clip | HEVC+AAC válido, sin temporales sueltos |
| Watchdog | Barra muerta revivida en un tick |

### Falta verificar (con ratón/uso real)

- **Burbuja vertical**: rebote del muelle, tamaño y distribución 3×2 de los
  botones. Los clics sintéticos de prueba necesitan pulsación mantenida; con
  ratón real debería ir fino, pero no está confirmado.
- **Sliders verticales** de volumen, brillo y opacidad dentro de la burbuja.
- **Vista de medios**: nunca se ha probado con algo realmente sonando. Faltan
  por confirmar play/pausa, siguiente/anterior y que los botones se atenúen
  cuando la sesión dice que ese control no está disponible.
- **Espectro en la píldora**: tamaño de las barras y si el umbral de "hay audio"
  es el adecuado en uso normal.
- **Acento del sistema**: aplicado y visible en los bordes de ventana. Falta ver
  si algún sitio queda ilegible; `rice-accent.ps1 -Restore` deshace exactamente
  lo que había.

### Pendientes

- **Carátula del reproductor**: los bytes ya se descargan (`media::cover_bytes`)
  pero no se decodifican ni se dibujan; la vista muestra un placeholder. Falta
  un decodificador PNG/JPEG y subirla como textura de egui.
- **Preview de vídeo**: SMTC no entrega fotogramas, así que habría que capturar
  la ventana del reproductor. Es un problema aparte del de la carátula.
- **Ocultar barras de título**: en apps que dibujan la suya (Vesktop, Discord,
  Claude, Zed) no se puede desde fuera; hay que usar el ajuste de cada app. En
  apps con marco nativo sí se podría quitar `WS_CAPTION`, pendiente de decidir
  si merece la pena.
- **AltSnap se traga el teclado si le descuadras la tecla Windows**: `Hotkeys=5B 5C`
  en `AltSnap.ini` hace que su modificador sea LWin/RWin, y AltSnap lleva su
  **propio** registro de si esa tecla está pulsada. Inyectar pulsaciones
  sintéticas de Win (`keybd_event`, `SendInput`) puede desincronizarlo: se queda
  creyendo que Super sigue abajo y a partir de ahí **intercepta teclas normales,
  la barra espaciadora incluida**, aunque el sistema no vea ningún modificador.
  Los síntomas son los tres a la vez: no se escriben espacios, `Win+Space` no
  abre la paleta y `Win+Alt+Space` no llega a GlazeWM. Diagnosticado con un hook
  propio: 86 pulsaciones de espacio llegaban al SO con `mods=[]`, así que ni
  teclado ni IME ni modificador atascado de verdad. **Recuperación: `Win+Shift+Z`**
  (mata AltSnap, que el supervisor relanza, y recarga AHK). Regla para el futuro:
  **no inyectar Win sintético para probar atajos** — usar IPC o la CLI.

- **`windeco` deja retazos y su restauración no es fiable**: se probó lanzándolo
  desde el supervisor y hubo que retirarlo el mismo día. Quitar
  `WS_CAPTION`/`WS_THICKFRAME` cambia el área cliente, y las apps que pintan su
  propio marco no se enteran: WezTerm (`window_decorations = 'RESIZE'`) y Firefox
  siguieron dibujando con el offset viejo y dejaron trozos de su barra de
  pestañas por la pantalla. El crate sigue en el árbol pero **no lo lanza nadie**.

  Peor: `--restore` **dejó a Vesktop sin marco durante horas**. El motivo fue un
  diagnóstico equivocado — di por hecho que "las apps Electron no tienen
  `WS_CAPTION`" y por eso no la conté como alterada. **Es falso**: medido, Discord
  (`0x14C70000`) y Legcord (`0x14C70000`) sí lo llevan, y Vesktop se había quedado
  en `0x160B0000`, sin `CAPTION` ni `THICKFRAME`, o sea sin botones de ventana.
  La forma correcta de comprobarlo es **comparar contra otra app de la misma
  familia**, no razonar sobre qué "suelen" hacer.

  Si se retoma: los estilos de ventana **no persisten**, así que reiniciar la app
  siempre los devuelve. En caliente hay que volver a poner los bits y forzar un
  `WM_SIZE` real (redimensionar un píxel y devolverla); `wm-redraw` de GlazeWM no
  basta.
- **Exportar clips en AV1** para compartirlos más pequeños.
- **Pomatez** (pomodoro en Electron) → temporizador en la isla.
- **Click-through con LoL en borderless**: NO resuelto. El mecanismo funciona y
  está verificado (la barra recibe `WS_EX_TRANSPARENT` sólo en el monitor del
  juego), pero el clic derecho seguía sin llegar al juego. Se usa LoL en
  *fullscreen* como solución. Sospecha: el juego lee entrada por raw input.

### Bandeja del sistema: cómo se resolvió

En Windows 11 25H2 la bandeja legacy ya no existe: `TrayNotifyWnd` sigue ahí
pero **sin `SysPager`/`ToolbarWindow32` dentro**, y los iconos los pinta XAML.
El truco clásico de leer `TBBUTTON` con `ReadProcessMemory` no tiene nada que
leer.

Se sondeó **UI Automation** y sí ve la bandeja: 28 botones, con
`AutomationId` = `NotifyItemIcon` (apps) o `SystemTrayIcon` (sistema), y el
tooltip completo en `Name`. Posición, nombre e invocación están disponibles.

**Lo que UIA no da son los píxeles del icono.** La salida planteada es mover la
barra de tareas fuera de pantalla en vez de ocultarla (`SetWindowPos` a y=-100),
de forma que siga siendo enumerable y capturable con `BitBlt`, y reenviar los
clics con UIA `Invoke`. Límites conocidos: el menú contextual del clic derecho
se abriría en la posición real (fuera de pantalla), y la mayoría de los iconos
viven en el desbordamiento, que es otro island XAML que hay que abrir.
