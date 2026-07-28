// Ajustes de memoria de Firefox. Se aplican al ARRANCAR Firefox; borrar este
// archivo lo deja todo como estaba (prefs.js recupera los valores por defecto
// en el siguiente arranque tras borrarlo).
//
// Medido en este equipo antes de escribirlo: 28 pestañas, 20 dominios
// distintos, 19 procesos de contenido, 7,0 GB. Y lo que NO es la causa, leído
// del código de mozilla-central: todas las fórmulas de Firefox que escalan con
// la RAM física se saturan muy por debajo de 32 GB.
//
//   browser.cache.memory.capacity -> tope de 32 MB, que ya vincula a los 5 GB
//   browser.sessionhistory.max_total_viewers -> tope de 8, vincula a 1 GB
//   image.mem.surfacecache.size_factor -> inerte por encima de ~7,9 GB
//
// Es decir: tocar la caché de memoria no habría servido de nada. Lo que sí
// cuesta memoria es (a) cuántos procesos hay y (b) cuánta holgura se queda cada
// uno. Todo lo de abajo ataca una de esas dos.

// ---------------------------------------------------------------- procesos

// El único cambio grande. Fission da un proceso por sitio distinto y no tiene
// tope: 20 dominios abiertos son 19 procesos, cada uno con su heap, su JIT y su
// coste base. La estrategia 2 ("IsolateHighValue") conserva el aislamiento para
// los sitios que importan -- aquellos donde hay sesión iniciada, credenciales
// guardadas o permisos COOP -- y comparte proceso para el resto.
//
// Muy preferible a apagar Fission (`fission.autostart=false`), que quitaría la
// protección contra ataques tipo Spectre en TODOS los sitios. Eso no se toca.
user_pref("fission.webContentIsolationStrategy", 2);

// Procesos por sitio. Con 4, un solo sitio con varios subdominios puede abrir
// cuatro. Con 2 sigue habiendo paralelismo y aislamiento.
// Coste: un sitio muy pesado con muchos subdominios serializa más.
user_pref("dom.ipc.processCount.webIsolated", 2);

// Firefox mantiene 3 procesos vacíos precargados para que abrir un sitio nuevo
// pinte antes. Son tres procesos parados ocupando memoria todo el rato.
// Coste real: el primer pintado al navegar a un dominio nuevo es algo más lento.
user_pref("dom.ipc.processPrelaunch.enabled", false);

// ------------------------------------------------- caché de imágenes por proceso

// El techo por defecto son ~1,98 GiB POR PROCESO. Con 19 procesos eso es un
// techo agregado absurdo. 256 MB por proceso sigue siendo holgado.
// Coste: volver a decodificar al hacer scroll hacia atrás, al cambiar de
// pestaña o al hacer zoom. Se nota sobre todo en SVG y GIF/WebP animados
// grandes. Necesita reiniciar Firefox (`mirror: once`).
user_pref("image.mem.surfacecache.max_size_kb", 262144);

// ------------------------------------------------------------- recolector de JS

// Cuánta holgura se deja el heap tras recolectar. 300 = 3x el tamaño vivo.
// Bajarlo a 2x es el ajuste con más efecto sobre la memoria residente de JS.
// Coste: más recolecciones, y por tanto más tirones en sitios con mucho JS.
user_pref("javascript.options.mem.gc_high_frequency_small_heap_growth", 200);

// Lo mismo fuera del modo de alta frecuencia. 150 -> 125.
// Ojo: por debajo de ~118 SpiderMonkey rechaza el valor, no lo baja más.
user_pref("javascript.options.mem.gc_low_frequency_heap_growth", 125);

// Las zonas por debajo de este tamaño normalmente no se recolectan nunca.
// Con muchas pestañas pequeñas eso se acumula. 27 MB -> 10 MB.
user_pref("javascript.options.mem.gc_allocation_threshold_mb", 10);

// Techo del nursery (la generación joven), POR PROCESO. 64 MB x 19 procesos es
// mucho techo. Coste: recolecciones menores más frecuentes, que son baratas.
user_pref("javascript.options.mem.nursery.max_kb", 16384);

// -------------------------------------------------------------- bfcache

// Páginas guardadas enteras para que atrás/adelante sea instantáneo, con un
// tope de 8 para todas las pestañas juntas. No se baja el número: se hace que
// caduquen antes. 30 min -> 5 min.
// Coste: volver atrás tras cinco minutos recarga la página.
user_pref("browser.sessionhistory.contentViewerTimeout", 300);

// NO se toca `browser.cache.memory.capacity`: su techo real son 32 MB de los
// 8,2 GB. Cambiarlo es ruido.
// NO se toca `fission.autostart`: quitar el aislamiento por seguridad para
// ahorrar memoria es un mal cambio.
// NO se toca `config.trim_on_minimize` ni nada parecido: se eliminó en Firefox
// 53, y la razón que dio Mozilla es que vaciar el working set no reduce la
// memoria comprometida y encima hay que volver a paginarla luego.
