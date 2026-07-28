//! Comandos: lo que no es ni una aplicación instalada ni un archivo.
//!
//! Dos cosas distintas, y conviene no mezclarlas:
//!
//! * **Los fijos** ([`builtin`]) son acciones del sistema que no tienen acceso
//!   directo en el menú Inicio -- bloquear, suspender, apagar, el administrador
//!   de dispositivos. Entran en el índice como una entrada más, así que los
//!   busca el mismo emparejador difuso y los lanza el mismo código: no hay una
//!   segunda lista con su propia ordenación ni su propia forma de fallar.
//!
//! * **El de ejecutar** ([`Runner`]) es dinámico: depende de lo que haya escrito
//!   el usuario, así que no puede vivir en el índice. Es el reemplazo de Win+R.
//!
//! El PATH se recorre UNA vez al arrancar y no en cada tecla. Resolver a mano
//! son unos 30 directorios por cuatro extensiones de `PATHEXT` -- más de cien
//! consultas al disco por pulsación, y en la letra equivocada un directorio de
//! red que tarda un segundo. Un mapa hecho al principio lo convierte en una
//! búsqueda en tabla.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::index::{Action, Entry};

/// Acciones del sistema, expresadas como lo que el shell ya sabe ejecutar.
///
/// Nombres en español porque es el idioma de la caja, con las palabras en
/// inglés metidas en `keywords`: teclear `lock` o `bloquear` encuentra lo mismo.
pub fn builtin() -> Vec<Entry> {
    const LIST: &[(&str, &str, &str, &str)] = &[
        // nombre, ejecutable, argumentos, palabras clave
        ("Bloquear la sesión", "rundll32.exe", "user32.dll,LockWorkStation", "lock bloquear pantalla"),
        // 0,1,0 = suspender (no hibernar), sin forzar, sin deshabilitar los
        // eventos de reactivación.
        ("Suspender", "rundll32.exe", "powrprof.dll,SetSuspendState 0,1,0", "sleep suspender dormir"),
        ("Apagar el equipo", "shutdown.exe", "/s /t 0", "shutdown apagar"),
        ("Reiniciar el equipo", "shutdown.exe", "/r /t 0", "restart reiniciar reboot"),
        ("Cerrar sesión", "shutdown.exe", "/l", "logoff cerrar sesion signout"),
        ("Administrador de tareas", "taskmgr.exe", "", "task manager tareas procesos"),
        ("Administrador de dispositivos", "devmgmt.msc", "", "device manager dispositivos hardware"),
        ("Servicios", "services.msc", "", "services servicios"),
        ("Editor del registro", "regedit.exe", "", "registry registro regedit"),
        ("Panel de control", "control.exe", "", "control panel panel de control"),
        ("Configuración de Windows", "ms-settings:", "", "settings configuracion ajustes"),
        ("Configuración de sonido", "ms-settings:sound", "", "sound sonido audio volumen"),
        ("Configuración de Bluetooth", "ms-settings:bluetooth", "", "bluetooth dispositivos"),
        ("Configuración de red", "ms-settings:network", "", "network red wifi internet"),
        ("Aplicaciones instaladas", "ms-settings:appsfeatures", "", "apps aplicaciones desinstalar programas"),
        ("Papelera de reciclaje", "shell:RecycleBinFolder", "", "recycle bin papelera basura"),
        ("Carpeta de inicio", "shell:Startup", "", "startup inicio autostart arranque"),
        ("Descargas", "shell:Downloads", "", "downloads descargas"),
        // Las del propio escritorio. Editar rice.json es lo que más se hace de
        // todo esto, y hasta ahora había que ir a buscarlo.
        ("Editar rice.json", "notepad.exe", r"%USERPROFILE%\.config\rice.json", "rice config configuracion json"),
        // El modelo local. Arrancar tarda ~50 s y ocupa 10 GB de VRAM y 15 de
        // RAM, asi que se enciende y se apaga a mano en vez de dejarlo residente.
        (
            "Arrancar el modelo local",
            "pwsh.exe",
            r"-NoProfile -File %USERPROFILE%\.config\rice-llm.ps1",
            "llm ia ai qwen modelo local llama jarvis arrancar",
        ),
        (
            "Parar el modelo local",
            "pwsh.exe",
            r"-NoProfile -File %USERPROFILE%\.config\rice-llm.ps1 -Stop",
            "llm ia ai qwen modelo local llama jarvis parar detener",
        ),
        // Va por wezterm porque el script pregunta: enseña la lista, espera un
        // número y luego una confirmación. Lanzarlo con ShellExecuteW a secas lo
        // dejaría sin ningún sitio donde escribir la respuesta.
        (
            "Desinstalar aplicaciones",
            r"%ProgramFiles%\WezTerm\wezterm-gui.exe",
            r"start -- pwsh -NoLogo -NoExit -File %USERPROFILE%\.config\rice-uninstall.ps1 -Size",
            "uninstall desinstalar quitar borrar programas apps remove",
        ),
        ("Recargar la configuración de GlazeWM", "glazewm.exe", "command wm-reload-config", "glazewm reload recargar wm"),
    ];

    LIST.iter()
        .map(|(name, target, args, keys)| Entry {
            name: (*name).to_string(),
            keywords: (*keys).to_string(),
            action: Action::Command {
                target: expand(target),
                args: expand(args),
            },
        })
        .collect()
}

/// Sustituye `%VAR%`. `shell:` y `ms-settings:` los resuelve el propio shell,
/// pero una ruta con `%USERPROFILE%` dentro de los ARGUMENTOS no la expande
/// nadie: llegaría literal al programa.
fn expand(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        match rest[i + 1..].find('%') {
            Some(j) => {
                let name = &rest[i + 1..i + 1 + j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &rest[i + 2 + j..];
            }
            None => {
                out.push_str(&rest[i..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

// -------------------------------------------------------------------- Win+R

/// Lo que se ofrecería ejecutar para la consulta actual.
#[derive(Clone)]
pub struct Run {
    /// Lo que se enseña en la fila.
    pub label: String,
    /// Por qué se ofrece: "en el PATH", "dirección web"...
    pub hint: String,
    pub target: String,
    pub args: String,
}

/// Todo lo ejecutable que hay en el PATH, por nombre sin extensión.
pub struct Runner {
    exes: HashMap<String, PathBuf>,
}

impl Runner {
    pub fn new() -> Self {
        let mut exes = HashMap::new();
        let exts: Vec<String> = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(|e| e.trim_start_matches('.').to_lowercase())
            .collect();
        for dir in std::env::var("PATH").unwrap_or_default().split(';') {
            if dir.is_empty() {
                continue;
            }
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let (Some(stem), Some(ext)) = (p.file_stem(), p.extension()) else { continue };
                let ext = ext.to_string_lossy().to_lowercase();
                if !exts.contains(&ext) {
                    continue;
                }
                // El primero que aparece gana, que es como resuelve Windows:
                // los directorios del PATH se miran en orden.
                exes.entry(stem.to_string_lossy().to_lowercase()).or_insert(p);
            }
        }
        Self { exes }
    }

    pub fn len(&self) -> usize {
        self.exes.len()
    }

    /// La ruta completa de un ejecutable del PATH.
    ///
    /// La necesitan los iconos: al shell se le puede pedir el icono de
    /// `C:\Windows\System32\shutdown.exe`, pero de `shutdown.exe` a secas no --
    /// lo resuelve contra el directorio actual y no encuentra nada.
    pub fn resolve(&self, name: &str) -> Option<&PathBuf> {
        let key = name.to_lowercase();
        self.exes.get(key.strip_suffix(".exe").unwrap_or(&key))
    }

    /// ¿Hay algo que ejecutar en lo que se está escribiendo?
    ///
    /// El orden importa: `>` primero porque es explícito y tiene que ganar
    /// siempre, y las direcciones web antes que el PATH porque `http` podría
    /// ser el nombre de algún ejecutable.
    pub fn offer(&self, query: &str) -> Option<Run> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }

        // `>` = ejecútalo tal cual, sin preguntarme si existe. La vía de escape
        // para todo lo que las reglas de abajo no reconocen.
        if let Some(rest) = q.strip_prefix('>') {
            let rest = rest.trim();
            if rest.is_empty() {
                return None;
            }
            let (target, args) = split_cmd(rest);
            return Some(Run {
                label: format!("Ejecutar: {rest}"),
                hint: "sin comprobar".into(),
                target,
                args,
            });
        }

        if q.starts_with("http://") || q.starts_with("https://") || q.starts_with("www.") {
            let url = if q.starts_with("www.") { format!("https://{q}") } else { q.to_string() };
            return Some(Run {
                label: format!("Abrir {q}"),
                hint: "dirección web".into(),
                target: url,
                args: String::new(),
            });
        }

        let (first, args) = split_cmd(q);

        // Una ruta escrita a mano, con o sin comillas.
        let as_path = PathBuf::from(first.trim_matches('"'));
        if (first.contains('\\') || first.contains('/')) && as_path.exists() {
            return Some(Run {
                label: format!("Abrir {}", as_path.display()),
                hint: "ruta".into(),
                target: as_path.to_string_lossy().into_owned(),
                args,
            });
        }

        // Un ejecutable del PATH. Se prueba tal cual y sin extensión, para que
        // tanto `pwsh` como `pwsh.exe` valgan.
        let key = first.to_lowercase();
        let key = key.strip_suffix(".exe").unwrap_or(&key);
        let hit = self.exes.get(key)?;
        Some(Run {
            label: if args.is_empty() { format!("Ejecutar {first}") } else { format!("Ejecutar {first} {args}") },
            hint: hit.to_string_lossy().into_owned(),
            target: hit.to_string_lossy().into_owned(),
            args,
        })
    }
}

/// Parte "programa argumentos" respetando un primer argumento entrecomillado.
fn split_cmd(s: &str) -> (String, String) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    match s.find(char::is_whitespace) {
        Some(i) => (s[..i].to_string(), s[i..].trim().to_string()),
        None => (s.to_string(), String::new()),
    }
}
