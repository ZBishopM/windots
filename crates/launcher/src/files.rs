//! El índice de archivos: por qué es nuestro y no de otro.
//!
//! # La decisión
//!
//! Buscar archivos al instante necesita un índice. En Windows había tres formas
//! de tener uno, y las tres se midieron en ESTA máquina antes de elegir.
//!
//! ## 1. El índice de Windows (servicio WSearch, catálogo SystemIndex)
//!
//! Ya está corriendo, no pide administrador, y se consulta con SQL a través del
//! proveedor OLE DB `Search.CollatorDSO`. Sobre el papel es la opción obvia.
//! Medido, no lo es:
//!
//!   - **Cobertura del 12%.** El catálogo tiene ~196.000 elementos. Bajo el
//!     perfil hay 1.394.364 entradas y en las otras unidades ~776.000 más. El
//!     ámbito de rastreo excluye `AppData`, `.config`, `dotfiles`, `.claude`,
//!     `.cargo`, `.rustup`, `.vscode` y de hecho `Users\obisp\.*\` entero --
//!     todos los dotfolders. **D: no está en el ámbito en absoluto.** Excluye
//!     justo donde vive el trabajo.
//!   - **No hay infijos.** Es la razón de peso. El índice sólo tiene entrada
//!     invertida por prefijo: `CONTAINS(System.FileName,'algo*')` responde en
//!     4-9 ms, pero `LIKE '%algo%'` cuesta 133-206 ms. Una búsqueda difusa es
//!     infija por definición -- teclear `mnrs` para encontrar `main.rs` es el
//!     caso normal, no el raro. Un lanzador difuso montado sobre un índice de
//!     sólo-prefijo deja de ser difuso.
//!   - **218 MB residentes.** SearchIndexer 173 MB más SearchProtocolHost y dos
//!     SearchFilterHost. Este escritorio existe porque la Command Palette
//!     costaba 267 MB; cambiarlos por 218 MB no es un cambio.
//!   - **Rutas localizadas.** `System.ItemPathDisplay` devuelve
//!     `C:\Usuarios\obisp\...`, que no existe en disco. Hay que pedir
//!     `System.ItemUrl` y deshacer el formato URL, o lanzar rutas que fallan.
//!
//! ## 2. Everything (voidtools) por IPC
//!
//! Técnicamente lo mejor: lee la MFT de NTFS y construye la base de un volumen
//! en ~1 segundo. Y el IPC es viable -- el "SDK" es un envoltorio de user32 con
//! fuente MIT, así que basta `FindWindowW` sobre la clase
//! `EVERYTHING_TASKBAR_NOTIFICATION` y un `WM_COPYDATA` con una
//! `EVERYTHING_IPC_QUERY2` empaquetada de 28 bytes; existe además el crate puro
//! `everything-ipc`. Nada de eso es el problema.
//!
//! El problema está debajo: **leer la MFT exige abrir el volumen en crudo, y eso
//! es administrador**. Por eso Everything instala un servicio. Sin ese servicio
//! cae a "folder indexes", que son escaneos recursivos de directorios --
//! exactamente lo que hace este archivo, pero con un programa de terceros y un
//! servicio permanente de por medio. Y no está instalado.
//!
//! Se descarta por dependencia, no por técnica. Queda el hueco documentado: si
//! algún día Everything está presente, puede entrar como otra fuente y este
//! recorrido pasa a ser el respaldo.
//!
//! ## 3. Índice propio (lo elegido)
//!
//! Un recorrido paralelo del árbol, en memoria, dentro del propio launcher. Sin
//! administrador, sin servicio, sin terceros; la cobertura es exactamente la que
//! digan `launcher.file_roots` y `file_skip`; y permite apagar WSearch y
//! recuperar sus 218 MB, que es el sentido de todo esto.
//!
//! # Cómo está hecho, y por qué así
//!
//! Las dos decisiones de implementación salieron de medir, no de suponer. El
//! primer intento usaba el crate [`nucleo`] (el selector completo de Helix) como
//! almacén y emparejador. Recorrió las 2.169.925 entradas de esta máquina en
//! **131 s ocupando 266 MB**. El mismo recorrido pelado -- contar y ya -- cuesta
//! **19 s y 10 MB**. Es decir: la maquinaria costaba 112 segundos y 256 MB. De
//! ahí las dos piezas de abajo.
//!
//! **El almacén es nuestro.** nucleo gasta ~118 bytes por elemento entre su
//! `Utf32String` y su contabilidad. Aquí una entrada son 12 bytes -- offset,
//! longitud y el directorio que la contiene -- más su nombre en una arena
//! compartida, unos 26 bytes en total. Las rutas completas no se guardan: cada
//! directorio se interna una vez y apunta a su padre, así que reconstruir una
//! ruta es subir por la cadena. Guardarlas enteras serían 225 MB de texto
//! medidos.
//!
//! **El reparto de trabajo es local.** La primera versión tenía una pila global
//! con un `Condvar`, y cada directorio terminado despertaba a los ocho hilos;
//! con ~300.000 directorios eso es millones de despertares peleándose por el
//! mismo candado. Ahora cada hilo tiene su propia pila y su propio trozo de
//! arena, y sólo toca lo compartido cuando se le acumula trabajo de sobra o
//! cuando cierra un trozo.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Duration;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// Raíz: no tiene padre y su nombre es la ruta entera (`C:\`).
const NO_PARENT: u32 = u32::MAX;

/// Bit 31 del campo `dir` de una entrada: la entrada es ella misma un
/// directorio. Los otros 31 bits son el id del directorio que la contiene, igual
/// para archivos y para carpetas, así que reconstruir la ruta es la misma cuenta
/// en los dos casos.
const IS_DIR: u32 = 1 << 31;

/// Entradas por trozo de arena. Es a la vez la unidad de reparto al buscar (con
/// ~130 trozos hay grano de sobra para ocho hilos) y el retardo con el que lo
/// recién recorrido se vuelve buscable.
const CHUNK: usize = 16_384;

// ------------------------------------------------------------------- almacén

#[derive(Clone, Copy)]
struct Ent {
    /// Offset del nombre dentro de `Chunk::names`.
    off: u32,
    len: u16,
    /// Cuánto vale el sitio donde vive, calculado al recorrer (ver
    /// [`dir_penalty`]). Va aquí y no en la tabla de directorios porque `off` y
    /// `len` dejaban dos bytes de relleno: sale gratis, y consultarlo al puntuar
    /// no toma ningún candado.
    w: i16,
    /// Id del directorio contenedor, con `IS_DIR` en el bit alto.
    dir: u32,
}

/// Un trozo cerrado: nombres empaquetados y sus entradas. Inmutable una vez
/// sellado, que es lo que permite compartirlo con los hilos de búsqueda sin
/// candado.
#[derive(Default)]
struct Chunk {
    names: Vec<u8>,
    ents: Vec<Ent>,
}

impl Chunk {
    fn push(&mut self, name: &str, dir: u32, w: i16) {
        // Un nombre de archivo en NTFS no pasa de 255 caracteres, así que u16
        // sobra; truncar aquí sería un nombre corrupto, no un nombre largo.
        if name.len() > u16::MAX as usize {
            return;
        }
        self.ents.push(Ent { off: self.names.len() as u32, len: name.len() as u16, w, dir });
        self.names.extend_from_slice(name.as_bytes());
    }

    fn name(&self, e: &Ent) -> &str {
        // Seguro: sólo se escribe desde `push`, que copia un `&str`.
        unsafe {
            std::str::from_utf8_unchecked(
                &self.names[e.off as usize..(e.off as usize + e.len as usize)],
            )
        }
    }
}

#[derive(Default)]
struct Store {
    sealed: RwLock<Vec<Arc<Chunk>>>,
}

impl Store {
    fn seal(&self, mut c: Chunk) {
        if c.ents.is_empty() {
            return;
        }
        // Los dos vectores crecieron duplicandose, asi que pueden llevar hasta
        // el doble de lo que usan. Un trozo sellado ya no cambia nunca: es el
        // momento exacto de devolver esa mitad.
        c.names.shrink_to_fit();
        c.ents.shrink_to_fit();
        self.sealed.write().unwrap().push(Arc::new(c));
    }

    /// Copia barata de la lista de trozos: sólo punteros, y suelta el candado
    /// antes de ponerse a buscar.
    fn snapshot(&self) -> Vec<Arc<Chunk>> {
        self.sealed.read().unwrap().clone()
    }
}

// --------------------------------------------------------------- directorios

/// Tabla de directorios internados.
pub struct Dirs {
    /// Todos los nombres, concatenados.
    buf: String,
    /// Por directorio: (padre, offset en `buf`, longitud).
    rec: Vec<(u32, u32, u32)>,
    /// (padre, nombre en minúsculas) -> id.
    ///
    /// Vacío durante el recorrido: éste crea los hijos sabiendo ya el id del
    /// padre y nunca consulta. Se construye de una vez al terminar, para el
    /// vigilante, que sí ve rutas sueltas y tiene que resolver directorios que
    /// ya existen en lugar de duplicarlos. Construirlo durante el recorrido
    /// costaba un `to_lowercase` y una inserción por directorio bajo el candado
    /// de escritura.
    idx: HashMap<(u32, Box<str>), u32>,
}

impl Dirs {
    fn new() -> Self {
        Self { buf: String::new(), rec: Vec::new(), idx: HashMap::new() }
    }

    /// Añade sin comprobar duplicados. Para el recorrido.
    fn add(&mut self, parent: u32, name: &str) -> u32 {
        let off = self.buf.len() as u32;
        self.buf.push_str(name);
        let id = self.rec.len() as u32;
        self.rec.push((parent, off, name.len() as u32));
        id
    }

    /// Devuelve el id, creándolo si hace falta. Para el vigilante.
    fn intern(&mut self, parent: u32, name: &str) -> u32 {
        let key = (parent, name.to_lowercase().into_boxed_str());
        if let Some(&id) = self.idx.get(&key) {
            return id;
        }
        let id = self.add(parent, name);
        self.idx.insert(key, id);
        id
    }

    fn build_index(&mut self) {
        self.buf.shrink_to_fit();
        self.rec.shrink_to_fit();
        self.idx.reserve(self.rec.len());
        for id in 0..self.rec.len() as u32 {
            let (parent, off, len) = self.rec[id as usize];
            let name = self.buf[off as usize..(off + len) as usize].to_lowercase();
            self.idx.entry((parent, name.into_boxed_str())).or_insert(id);
        }
    }

    fn name(&self, id: u32) -> &str {
        let (_, off, len) = self.rec[id as usize];
        &self.buf[off as usize..(off + len) as usize]
    }

    /// Reconstruye la ruta completa subiendo por los padres.
    pub fn path(&self, id: u32) -> String {
        let mut parts = Vec::new();
        let mut cur = id;
        loop {
            parts.push(self.name(cur));
            let parent = self.rec[cur as usize].0;
            if parent == NO_PARENT {
                break;
            }
            cur = parent;
        }
        parts.reverse();
        // La raíz ya trae su separador final (`C:\`), el resto no.
        let mut out = String::from(parts[0]);
        for p in &parts[1..] {
            if !out.ends_with('\\') {
                out.push('\\');
            }
            out.push_str(p);
        }
        out
    }
}

// -------------------------------------------------------------------- índice

/// Un resultado listo para pintar y para lanzar.
#[derive(Clone)]
pub struct FileHit {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// Identidad de una entrada, para tacharla cuando el vigilante la ve
/// desaparecer. Borrar de verdad exigiría compactar la arena con los hilos de
/// búsqueda leyéndola.
fn ident(dir: u32, name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        h ^= b.to_ascii_lowercase() as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h ^ ((dir as u64) << 32)
}

pub struct FileIndex {
    dirs: Arc<RwLock<Dirs>>,
    scanning: Arc<AtomicBool>,
    count: Arc<AtomicUsize>,
    /// Generación de la consulta en curso. Cualquier búsqueda cuyo número ya no
    /// coincida se abandona a medias: al teclear rápido llegan más consultas que
    /// las que da tiempo a resolver, y terminar las viejas es trabajo tirado.
    gen: Arc<AtomicU64>,
    /// (generación que los produjo, resultados). Guardar el número permite
    /// distinguir "todavía buscando" de "no hay nada", que en pantalla son cosas
    /// muy distintas.
    out: Arc<Mutex<(u64, Vec<FileHit>)>>,
    tx: Sender<(u64, String, usize)>,
}

impl FileIndex {
    /// Arranca el recorrido de fondo y devuelve el índice ya consultable.
    ///
    /// `repaint` es lo que se llama cuando hay resultados nuevos. La búsqueda no
    /// pasa por el hilo de la interfaz -- un barrido completo son decenas de
    /// milisegundos y hacerlo en `update()` sería tirar fotogramas en cada
    /// tecla.
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::start(repaint, Duration::from_secs(12))
    }

    /// Sin la espera inicial. Para medir: los 12 segundos son para no pelear con
    /// el arranque de sesión, y en un banco de pruebas sólo son 12 segundos.
    pub fn start_now(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::start(repaint, Duration::ZERO)
    }

    fn start(repaint: Arc<dyn Fn() + Send + Sync>, delay: Duration) -> Self {
        let cfg = Cfg::of();
        let store = Arc::new(Store::default());
        let dirs = Arc::new(RwLock::new(Dirs::new()));
        let gone = Arc::new(RwLock::new(HashSet::new()));
        let scanning = Arc::new(AtomicBool::new(true));
        let count = Arc::new(AtomicUsize::new(0));
        let gen = Arc::new(AtomicU64::new(0));
        let out = Arc::new(Mutex::new((0, Vec::new())));
        let (tx, rx) = std::sync::mpsc::channel();

        {
            let (store, dirs, gone, scanning, count) =
                (store.clone(), dirs.clone(), gone.clone(), scanning.clone(), count.clone());
            std::thread::Builder::new()
                .name("file-index".into())
                .spawn(move || {
                    // El launcher arranca con la sesión, y en ese momento el
                    // disco es el recurso escaso: el arranque de este equipo se
                    // bajó de 121,9 s a 104,5 s peleando justo por eso. Ocho
                    // hilos recorriendo dos millones de entradas ahí dentro
                    // devolverían parte de lo ganado. Se espera a que la sesión
                    // se asiente; nadie busca archivos en los primeros segundos.
                    std::thread::sleep(delay);
                    walk_all(&cfg, &store, &dirs, &count);
                    dirs.write().unwrap().build_index();
                    scanning.store(false, Ordering::Release);
                    // Hand the walk's peak back to the system, once.
                    //
                    // The arenas are already shrunk to fit, but the heap does not
                    // return freed pages on its own: measured, the process sat at
                    // 223 MB after a walk whose result is ~117 MB. The rest was
                    // the job stack, the lowercased paths and the buckets the
                    // directory map grew through. The periodic trim in the UI
                    // loop cannot do this -- it only runs on idle frames, and an
                    // idle launcher asks for one an hour.
                    rice_common::win::trim_ram();
                    // Los vigilantes sólo tienen sentido sobre un árbol ya
                    // recorrido: antes, los avisos hablarían de directorios cuyo
                    // id todavía no existe.
                    watch_all(&cfg, store, dirs, count, gone);
                })
                .ok();
        }
        {
            let (store, dirs, gone, gen, out) =
                (store.clone(), dirs.clone(), gone.clone(), gen.clone(), out.clone());
            std::thread::Builder::new()
                .name("file-search".into())
                .spawn(move || search_loop(rx, store, dirs, gone, gen, out, repaint))
                .ok();
        }

        Self { dirs, scanning, count, gen, out, tx }
    }

    /// ¿Sigue creciendo el índice?
    pub fn scanning(&self) -> bool {
        self.scanning.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    /// Pide una búsqueda. Vuelve enseguida; los resultados aparecen en
    /// [`results`](Self::results) y llega un repintado cuando están.
    pub fn search(&self, q: &str, max: usize) {
        let g = self.gen.fetch_add(1, Ordering::SeqCst) + 1;
        if q.trim().is_empty() {
            *self.out.lock().unwrap() = (g, Vec::new());
            return;
        }
        let _ = self.tx.send((g, q.to_string(), max));
    }

    /// Lo último que terminó de buscarse.
    pub fn results(&self) -> Vec<FileHit> {
        self.out.lock().unwrap().1.clone()
    }

    /// ¿Los resultados son de la consulta actual, o de la anterior?
    pub fn settled(&self) -> bool {
        self.out.lock().unwrap().0 == self.gen.load(Ordering::SeqCst)
    }

    /// Reconstruye la ruta de un directorio. Para el resto del programa.
    pub fn dir_path(&self, id: u32) -> String {
        self.dirs.read().unwrap().path(id)
    }
}

// ------------------------------------------------------------------ búsqueda

fn search_loop(
    rx: Receiver<(u64, String, usize)>,
    store: Arc<Store>,
    dirs: Arc<RwLock<Dirs>>,
    gone: Arc<RwLock<HashSet<u64>>>,
    gen: Arc<AtomicU64>,
    out: Arc<Mutex<(u64, Vec<FileHit>)>>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    while let Ok(mut job) = rx.recv() {
        // Al teclear deprisa se apilan consultas; sólo interesa la última.
        while let Ok(newer) = rx.try_recv() {
            job = newer;
        }
        let (g, query, max) = job;
        if g != gen.load(Ordering::SeqCst) {
            continue;
        }
        let hits = scan(&store, &query, max, &gen, g);
        if g != gen.load(Ordering::SeqCst) {
            continue;
        }

        // Las rutas se construyen sólo para lo que se va a enseñar: subir por la
        // cadena de padres para dos millones de entradas sería absurdo.
        let d = dirs.read().unwrap();
        let dead = gone.read().unwrap();
        let mut list = Vec::with_capacity(hits.len());
        for (dir, name) in hits {
            if dead.contains(&ident(dir, &name)) {
                continue;
            }
            let mut path = d.path(dir & !IS_DIR);
            if !path.ends_with('\\') {
                path.push('\\');
            }
            path.push_str(&name);
            list.push(FileHit { name, path, is_dir: dir & IS_DIR != 0 });
        }
        drop(dead);
        drop(d);

        *out.lock().unwrap() = (g, list);
        repaint();
    }
}

/// Los `max` mejores de todo el almacén.
fn scan(
    store: &Store,
    query: &str,
    max: usize,
    gen: &AtomicU64,
    mine: u64,
) -> Vec<(u32, String)> {
    let chunks = store.snapshot();
    if chunks.is_empty() {
        return Vec::new();
    }
    let pat = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    // El prefiltro trabaja sobre bytes en minúscula ASCII. Si la consulta trae
    // acentos no sirve y se desactiva: emparejar `ñ` byte a byte daría falsos
    // negativos, y un falso negativo aquí es un archivo que el usuario ve que
    // existe y la barra jura que no.
    let needle: Option<Vec<u8>> = query
        .is_ascii()
        .then(|| query.bytes().filter(|b| !b.is_ascii_whitespace()).map(|b| b.to_ascii_lowercase()).collect());
    let qlow = query.to_lowercase();

    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map(|p| p.get().min(8)).unwrap_or(4).max(1);

    let mut all: Vec<(i64, u32, String)> = std::thread::scope(|sc| {
        let hs: Vec<_> = (0..threads)
            .map(|_| {
                let (chunks, pat, needle, qlow, next) = (&chunks, &pat, &needle, &qlow, &next);
                sc.spawn(move || {
                    let mut m = Matcher::new(Config::DEFAULT);
                    let mut buf = Vec::new();
                    let mut top: Vec<(i64, u32, String)> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= chunks.len() {
                            break;
                        }
                        // Abandonar entre trozos: comprobarlo por entrada
                        // costaría más que emparejar.
                        if gen.load(Ordering::SeqCst) != mine {
                            break;
                        }
                        let c = &chunks[i];
                        for e in &c.ents {
                            let name = c.name(e);
                            if let Some(n) = needle {
                                if !subsequence(name.as_bytes(), n) {
                                    continue;
                                }
                            }
                            let hay = Utf32Str::new(name, &mut buf);
                            let Some(s) = pat.score(hay, &mut m) else { continue };
                            top.push((rank(s, name, qlow, e.w), e.dir, name.to_string()));
                        }
                        // Podar de vez en cuando en lugar de al final: una
                        // consulta de una letra empareja casi todo, y guardar
                        // dos millones de nombres para quedarse con nueve es
                        // justo la asignación de memoria que se quiere evitar.
                        if top.len() > max * 64 {
                            prune(&mut top, max * 8);
                        }
                    }
                    prune(&mut top, max * 8);
                    top
                })
            })
            .collect();
        hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });

    all.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.len().cmp(&b.2.len())));
    all.truncate(max);
    all.into_iter().map(|(_, dir, name)| (dir, name)).collect()
}

/// Deja los `keep` mejores, sin ordenar del todo.
fn prune(v: &mut Vec<(i64, u32, String)>, keep: usize) {
    if v.len() <= keep {
        return;
    }
    v.select_nth_unstable_by(keep, |a, b| b.0.cmp(&a.0));
    v.truncate(keep);
}

/// Puntuación final. La de nucleo mide lo bien que encajan los caracteres; esto
/// añade lo que el usuario espera de un lanzador y ella no sabe.
fn rank(base: u32, name: &str, qlow: &str, w: i16) -> i64 {
    // El peso del directorio pesa de verdad: ocho puntos por unidad lo pone en
    // la misma escala que los premios de abajo, asi que un encaje perfecto en
    // una cache de AppData pierde contra un encaje bueno en el perfil.
    let mut s = base as i64 * 4 + w as i64 * 8;
    let lower = name.to_lowercase();
    if lower == qlow {
        s += 4000;
    } else if lower.starts_with(qlow) {
        s += 800;
    } else if lower
        .rsplit_once('.')
        .map(|(stem, _)| stem == qlow)
        .unwrap_or(false)
    {
        // Escribir `main` debe encontrar `main.rs` antes que `maintenance.log`.
        s += 2000;
    }
    // A igualdad de encaje, el nombre más corto es el que se buscaba.
    s - name.len() as i64
}

/// ¿Están todos los bytes de `needle` en `hay`, en orden?
///
/// Descarta la inmensa mayoría de las entradas por unos pocos nanosegundos, muy
/// por debajo de lo que cuesta construir el `Utf32Str` y puntuar. Ante un byte
/// no ASCII se rinde y deja pasar: más vale puntuar de más que perder un
/// archivo con acentos.
fn subsequence(hay: &[u8], needle: &[u8]) -> bool {
    let mut i = 0;
    if needle.is_empty() {
        return true;
    }
    for &h in hay {
        if h >= 0x80 {
            return true;
        }
        if h.to_ascii_lowercase() == needle[i] {
            i += 1;
            if i == needle.len() {
                return true;
            }
        }
    }
    false
}

// ------------------------------------------------------------------ recorrido

/// Lo que el recorrido necesita saber, resuelto una vez.
pub struct Cfg {
    roots: Vec<String>,
    skip: Vec<String>,
    limit: usize,
}

impl Cfg {
    pub fn of() -> Self {
        let s = rice_common::settings::Settings::live();
        let l = &s.launcher;
        let roots = if l.file_roots.is_empty() { fixed_drives() } else { l.file_roots.clone() };
        Self {
            skip: l.file_skip.iter().map(|x| x.to_lowercase()).collect(),
            limit: l.file_limit,
            roots,
        }
    }

    fn skipped(&self, path_lower: &str) -> bool {
        self.skip.iter().any(|s| path_lower.ends_with(s.as_str()))
    }
}

/// Cada unidad fija. Incluye C:, así que el perfil entra por ahí y no hace falta
/// añadirlo aparte -- estaría dos veces.
fn fixed_drives() -> Vec<String> {
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetLogicalDrives() -> u32;
            fn GetDriveTypeW(root: *const u16) -> u32;
        }
        const DRIVE_FIXED: u32 = 3;
        let mask = GetLogicalDrives();
        let mut out = Vec::new();
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            if GetDriveTypeW(w.as_ptr()) == DRIVE_FIXED {
                out.push(root);
            }
        }
        out
    }
    #[cfg(not(windows))]
    vec!["/".to_string()]
}

/// Un directorio pendiente: su id, su ruta ya construida y su peso.
///
/// La ruta viaja con el trabajo en vez de reconstruirse desde la tabla, porque
/// reconstruirla es subir por los padres tomando el candado de lectura una vez
/// por directorio. Llevarla puesta lo convierte en una concatenación.
type Job = (u32, String, i16);

/// Lo que resta un directorio a lo que hay dentro de él.
///
/// Sin esto, buscar `sy` devolvía cuatro carpetas `AutofillStates` de cachés de
/// navegador empotrado, porque el nombre encajaba perfecto. El emparejador
/// puntúa cómo de bien encajan los caracteres y no tiene forma de saber que
/// nadie ha querido nunca abrir nada de `AppData\Local\...\CefCache`. Esto se
/// calcula una sola vez, al crear el directorio, y viaja en la entrada.
///
/// El -1 por nivel es la mitad interesante: a igualdad de encaje gana lo que
/// está más cerca de la raíz, que es casi siempre lo que se buscaba.
fn dir_penalty(name: &str) -> i16 {
    let mut p = -1;
    let n = name.to_ascii_lowercase();
    p += match n.as_str() {
        "appdata" | "appdata.local" => -40,
        "windows" | "winsxs" | "driverstore" | "installer" => -35,
        "program files" | "program files (x86)" | "programdata" => -25,
        "cache" | "caches" | "cache2" | "cefcache" | "browsercache" | "gpucache" => -30,
        "temp" | "tmp" | "logs" | "crashpad" | "crashdumps" => -25,
        "steamapps" | "packages" | "windowsapps" | "assembly" => -15,
        _ => 0,
    };
    p
}

struct Queue {
    stack: Mutex<Vec<Job>>,
    cv: Condvar,
    /// Hilos con trabajo entre manos. El recorrido ha terminado cuando la pila
    /// global está vacía Y esto es cero; sin el contador, el hilo que saca el
    /// último elemento deja a los demás viendo una pila vacía y saliendo antes
    /// de que aparezcan los hijos.
    active: AtomicUsize,
}

/// Con más de esto acumulado en su pila local, un hilo cede la mitad. Bajo, y se
/// vuelve a la pila global compartida de antes; alto, y un hilo se queda con un
/// subárbol entero mientras los demás esperan.
const SPILL: usize = 48;

fn walk_all(cfg: &Cfg, store: &Arc<Store>, dirs: &Arc<RwLock<Dirs>>, count: &Arc<AtomicUsize>) {
    let q = Arc::new(Queue {
        stack: Mutex::new(Vec::new()),
        cv: Condvar::new(),
        active: AtomicUsize::new(0),
    });
    {
        let mut d = dirs.write().unwrap();
        let mut s = q.stack.lock().unwrap();
        for r in &cfg.roots {
            let id = d.add(NO_PARENT, r);
            s.push((id, r.clone(), 0));
        }
    }

    // El recorrido es de E/S, no de CPU: los hilos pasan el tiempo esperando al
    // disco, así que interesan al menos tantos como núcleos.
    let n = std::thread::available_parallelism().map(|p| p.get().min(8)).unwrap_or(4).max(2);
    std::thread::scope(|sc| {
        for _ in 0..n {
            let q = q.clone();
            sc.spawn(move || {
                background_priority();
                worker(cfg, &q, store, dirs, count)
            });
        }
    });
}

/// Baja la prioridad del hilo actual y le marca la E/S como de fondo.
///
/// `THREAD_MODE_BACKGROUND_BEGIN` no es lo mismo que bajar la prioridad: además
/// pone la E/S del hilo en la cola de baja prioridad del planificador de disco,
/// que es justo lo que hace falta aquí. El recorrido puede tardar el doble sin
/// que nadie lo note; lo que sí se nota es que le robe el disco a otra cosa.
fn background_priority() {
    #[cfg(windows)]
    unsafe {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(h: isize, p: i32) -> i32;
        }
        const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
        const THREAD_PRIORITY_LOWEST: i32 = -2;
        // El modo de fondo sólo vale sobre el propio hilo y falla si ya está
        // puesto; la prioridad baja es el respaldo si no lo acepta.
        if SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) == 0 {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
        }
    }
}

fn worker(
    cfg: &Cfg,
    q: &Arc<Queue>,
    store: &Arc<Store>,
    dirs: &Arc<RwLock<Dirs>>,
    count: &Arc<AtomicUsize>,
) {
    let mut local: Vec<Job> = Vec::new();
    let mut chunk = Chunk::default();
    let mut busy = false;

    loop {
        let job = match local.pop() {
            Some(j) => Some(j),
            None => {
                // Quedarse sin trabajo local es dejar de contar como activo, y
                // hay que anunciarlo: puede ser el último y los demás están
                // esperando para poder salir.
                if busy {
                    q.active.fetch_sub(1, Ordering::SeqCst);
                    busy = false;
                    q.cv.notify_all();
                }
                // Sacar de la pila global y declararse activo tiene que ocurrir
                // bajo el mismo candado, o entre las dos cosas otro hilo ve pila
                // vacía y cero activos y da el recorrido por terminado.
                let mut s = q.stack.lock().unwrap();
                loop {
                    if let Some(j) = s.pop() {
                        q.active.fetch_add(1, Ordering::SeqCst);
                        busy = true;
                        break Some(j);
                    }
                    if q.active.load(Ordering::SeqCst) == 0 {
                        break None;
                    }
                    s = q.cv.wait(s).unwrap();
                }
            }
        };
        let Some((id, path, w)) = job else {
            // Al salir hay que despertar a los demás: siguen dormidos esperando
            // trabajo que ya no va a llegar.
            q.cv.notify_all();
            break;
        };

        scan_dir(cfg, id, &path, w, &mut local, &mut chunk, dirs, count);

        if chunk.ents.len() >= CHUNK {
            store.seal(std::mem::take(&mut chunk));
        }
        // Ceder la mitad de lo acumulado. Se toma de abajo, que es lo más viejo
        // y por tanto lo más alto del árbol: subárboles grandes para los demás,
        // y este hilo sigue con lo profundo que ya tiene en caché.
        if local.len() > SPILL {
            let half = local.len() / 2;
            let give: Vec<Job> = local.drain(..half).collect();
            q.stack.lock().unwrap().extend(give);
            q.cv.notify_all();
        }
    }
    store.seal(chunk);
}

fn scan_dir(
    cfg: &Cfg,
    id: u32,
    path: &str,
    w: i16,
    local: &mut Vec<Job>,
    chunk: &mut Chunk,
    dirs: &Arc<RwLock<Dirs>>,
    count: &Arc<AtomicUsize>,
) {
    let Ok(rd) = std::fs::read_dir(path) else { return };
    let mut subdirs: Vec<(String, String)> = Vec::new();
    let mut n = 0usize;

    for e in rd.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        // En Windows esto no cuesta una llamada extra: read_dir ya trae los
        // atributos que devolvió FindFirstFileW.
        let Ok(md) = e.metadata() else { continue };
        n += 1;

        if !md.is_dir() {
            chunk.push(&name, id, w);
            continue;
        }

        chunk.push(&name, id | IS_DIR, w);

        // Los puntos de reanálisis (junctions, enlaces) se indexan pero no se
        // atraviesan: `C:\Users\obisp\Application Data` apunta a su propio
        // padre, y seguirlo es un bucle infinito.
        if is_reparse(&md) {
            continue;
        }
        let child = if path.ends_with('\\') {
            format!("{path}{name}")
        } else {
            format!("{path}\\{name}")
        };
        if cfg.skipped(&child.to_lowercase()) {
            continue;
        }
        subdirs.push((name, child));
    }

    if n > 0 && count.fetch_add(n, Ordering::Relaxed) + n >= cfg.limit {
        subdirs.clear();
    }
    if subdirs.is_empty() {
        return;
    }
    // Un solo bloqueo de escritura por directorio en vez de uno por hijo.
    let mut d = dirs.write().unwrap();
    for (name, child) in subdirs {
        let cw = w.saturating_add(dir_penalty(&name));
        let cid = d.add(id, &name);
        local.push((cid, child, cw));
    }
}

fn is_reparse(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    md.is_symlink()
}

/// El suelo del recorrido: recorrer y contar, sin internar, sin candados y sin
/// almacenar. Lo que el recorrido real gaste por encima de esto es coste que
/// hemos añadido nosotros, y por tanto coste que se puede quitar.
pub fn bench_floor() -> (usize, f32) {
    fn rec(path: &str, skip: &[String], n: &mut usize) {
        let Ok(rd) = std::fs::read_dir(path) else { return };
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            *n += 1;
            if !md.is_dir() || is_reparse(&md) {
                continue;
            }
            let Ok(name) = e.file_name().into_string() else { continue };
            let child = if path.ends_with('\\') {
                format!("{path}{name}")
            } else {
                format!("{path}\\{name}")
            };
            let lower = child.to_lowercase();
            if skip.iter().any(|s| lower.ends_with(s.as_str())) {
                continue;
            }
            rec(&child, skip, n);
        }
    }
    let cfg = Cfg::of();
    let t = std::time::Instant::now();
    let total: usize = std::thread::scope(|sc| {
        let hs: Vec<_> = cfg
            .roots
            .iter()
            .map(|r| {
                let skip = &cfg.skip;
                sc.spawn(move || {
                    let mut n = 0;
                    rec(r, skip, &mut n);
                    n
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap_or(0)).sum()
    });
    (total, t.elapsed().as_secs_f32())
}

// ------------------------------------------------------------------ vigilancia

/// Mantiene el índice al día sin volver a recorrer nada.
///
/// `ReadDirectoryChangesW` con `bWatchSubtree` es un solo descriptor por raíz
/// para todo su árbol, y no pide administrador. El hilo se queda bloqueado
/// dentro de la llamada: no hay sondeo ni temporizador, luego no hay coste
/// cuando no pasa nada.
#[cfg(windows)]
fn watch_all(
    cfg: &Cfg,
    store: Arc<Store>,
    dirs: Arc<RwLock<Dirs>>,
    count: Arc<AtomicUsize>,
    gone: Arc<RwLock<HashSet<u64>>>,
) {
    for root in cfg.roots.clone() {
        let (store, dirs, count, gone) = (store.clone(), dirs.clone(), count.clone(), gone.clone());
        let skip = cfg.skip.clone();
        std::thread::Builder::new()
            .name(format!("watch-{}", root.trim_end_matches(":\\")))
            .spawn(move || watch_root(&root, &skip, store, dirs, count, gone))
            .ok();
    }
}

#[cfg(not(windows))]
fn watch_all(
    _cfg: &Cfg,
    _store: Arc<Store>,
    _dirs: Arc<RwLock<Dirs>>,
    _count: Arc<AtomicUsize>,
    _gone: Arc<RwLock<HashSet<u64>>>,
) {
}

#[cfg(windows)]
fn watch_root(
    root: &str,
    skip: &[String],
    store: Arc<Store>,
    dirs: Arc<RwLock<Dirs>>,
    count: Arc<AtomicUsize>,
    gone: Arc<RwLock<HashSet<u64>>>,
) {
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            sa: isize,
            disp: u32,
            flags: u32,
            template: isize,
        ) -> isize;
        fn ReadDirectoryChangesW(
            h: isize,
            buf: *mut u8,
            len: u32,
            subtree: i32,
            filter: u32,
            returned: *mut u32,
            overlapped: isize,
            routine: isize,
        ) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    const FILE_LIST_DIRECTORY: u32 = 0x0001;
    const FILE_SHARE_ALL: u32 = 0x0007; // lectura | escritura | borrado
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000; // obligatorio para abrir un directorio
    const NOTIFY_NAME: u32 = 0x0000_0003; // nombre de archivo | nombre de directorio
    const INVALID: isize = -1;

    const ADDED: u32 = 1;
    const REMOVED: u32 = 2;
    const RENAMED_OLD: u32 = 4;
    const RENAMED_NEW: u32 = 5;

    let w: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_ALL,
            0,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            0,
        )
    };
    if h == 0 || h == INVALID {
        return;
    }

    // Lo que llega suelto se acumula aquí y se sella cuando toca, para no crear
    // un trozo de una entrada por cada archivo temporal que aparece.
    let mut pending = Chunk::default();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut got = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                h,
                buf.as_mut_ptr(),
                buf.len() as u32,
                1,
                NOTIFY_NAME,
                &mut got,
                0,
                0,
            )
        };
        if ok == 0 {
            break;
        }
        if got == 0 {
            // Desbordamiento del búfer del núcleo: hubo más cambios de los que
            // cabían y no se puede saber cuáles. Se sigue escuchando; lo perdido
            // se recupera en el siguiente arranque.
            continue;
        }

        let mut off = 0usize;
        loop {
            if off + 12 > got as usize {
                break;
            }
            let base = unsafe { buf.as_ptr().add(off) };
            let next = unsafe { std::ptr::read_unaligned(base as *const u32) } as usize;
            let action = unsafe { std::ptr::read_unaligned(base.add(4) as *const u32) };
            let name_bytes = unsafe { std::ptr::read_unaligned(base.add(8) as *const u32) } as usize;
            if off + 12 + name_bytes > got as usize {
                break;
            }
            let units =
                unsafe { std::slice::from_raw_parts(base.add(12) as *const u16, name_bytes / 2) };
            let rel = String::from_utf16_lossy(units);

            if let Some((dir, w)) = resolve(&rel, root, skip, &dirs) {
                let name = rel.rsplit('\\').next().unwrap_or(&rel).to_string();
                match action {
                    ADDED | RENAMED_NEW => {
                        count.fetch_add(1, Ordering::Relaxed);
                        pending.push(&name, dir, w);
                        // Si vuelve algo que estaba tachado, deja de estarlo.
                        let mut g = gone.write().unwrap();
                        g.remove(&ident(dir, &name));
                        g.remove(&ident(dir | IS_DIR, &name));
                    }
                    REMOVED | RENAMED_OLD => {
                        let mut g = gone.write().unwrap();
                        // No se sabe si era archivo o carpeta: el aviso no lo
                        // dice y hacer un stat de algo que ya no existe no
                        // responde. Se tachan las dos formas.
                        g.insert(ident(dir, &name));
                        g.insert(ident(dir | IS_DIR, &name));
                    }
                    _ => {}
                }
            }

            if next == 0 {
                break;
            }
            off += next;
        }

        if !pending.ents.is_empty() {
            store.seal(std::mem::take(&mut pending));
        }
    }
    unsafe { CloseHandle(h) };
}

/// Id del directorio que contiene `rel` (relativo a `root`), creando lo que
/// falte. `None` si cae en una rama excluida.
#[cfg(windows)]
fn resolve(rel: &str, root: &str, skip: &[String], dirs: &Arc<RwLock<Dirs>>) -> Option<(u32, i16)> {
    let parent_rel = match rel.rfind('\\') {
        Some(i) => &rel[..i],
        None => "",
    };
    let full = format!("{}\\{}", root.trim_end_matches('\\'), parent_rel).to_lowercase();
    if skip.iter().any(|s| full.contains(s.as_str())) {
        return None;
    }
    let mut d = dirs.write().unwrap();
    // La raíz ya existe: el recorrido la creó primero, así que esto la
    // encuentra en vez de duplicarla.
    let mut cur = d.intern(NO_PARENT, root);
    let mut w: i16 = 0;
    for seg in parent_rel.split('\\').filter(|s| !s.is_empty()) {
        cur = d.intern(cur, seg);
        w = w.saturating_add(dir_penalty(seg));
    }
    Some((cur, w))
}
