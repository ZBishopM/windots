// Las "manos": traduce una petición en español a una acción en el equipo.
//
// El modelo local NO ejecuta nada. Elige una herramienta de una lista cerrada y
// devuelve sus argumentos; este archivo es quien ejecuta, y solo sabe hacer las
// cuatro cosas de `HERRAMIENTAS`. Un modelo que se invente `rm -rf` no tiene por
// dónde: no existe una herramienta que reciba una orden de shell.
//
// Nada de esto lleva su propio índice. Las aplicaciones salen del mismo
// `launcher.exe` de Win+Space (`--abrir` / `--buscar`) y los archivos del índice
// que ese launcher ya tiene en memoria, preguntado por una tubería con nombre.
// Una segunda lista sería una lista desactualizada: al instalar o mover algo, la
// caja lo encontraría y las manos no.
//
//   node rice-manos.mjs "abre el firefox"
//   node rice-manos.mjs --seco "bloquea la sesión"   dice qué haría, no lo hace
//   node rice-manos.mjs --aprender "<frase>" "<destino>"
//   node rice-manos.mjs --alias                      lista lo aprendido

import { spawn } from 'node:child_process';
import fs from 'node:fs';
import process from 'node:process';

const SERVIDOR = process.env.MANOS_URL ?? 'http://127.0.0.1:8080';

// El árbol VIVO es `~/dev`, no `~/dotfiles`: es de donde salen todos los
// binarios del rice y de donde `sync.ps1` copia hacia el repo. Apuntar al del
// repo funcionaba pero dejaba a las manos usando una compilación que nadie más
// del escritorio usa.
const LAUNCHER = `${process.env.USERPROFILE}\\dev\\target\\release\\launcher.exe`;

// La tubería del launcher residente. Ver `crates/launcher/src/tuberia.rs`.
const TUBERIA = '\\\\.\\pipe\\rice-launcher-archivos';

// Los alias APRENDIDOS. Los de `ALIAS` vienen en el código; estos los añade
// `--aprender` cuando algo no se encuentra y se resuelve a mano. Separados a
// propósito: este archivo es de esta máquina y crece solo, el código es lo que
// se versiona y viaja a cualquier otra.
const APRENDIDOS = `${process.env.USERPROFILE}\\.config\\manos-alias.json`;

// Seis y no cuatro: una petición que falla al primer intento gasta una vuelta
// en el fallo, otra en `buscar_app`, otra en el `abrir` bueno y una cuarta en
// contestar. Con cuatro se quedaba sin turnos justo después de acertar.
const MAX_VUELTAS = 6;

// Ejecuta un programa SIN shell: los argumentos van como array, así que nada de
// lo que devuelva el modelo puede convertirse en otro comando.
function correr(exe, args) {
  return new Promise((resolve) => {
    const p = spawn(exe, args, { windowsHide: true });
    let out = '';
    let err = '';
    p.stdout.on('data', (d) => (out += d));
    p.stderr.on('data', (d) => (err += d));
    p.on('close', (code) => resolve({ code, out: out.trim(), err: err.trim() }));
    p.on('error', (e) => resolve({ code: -1, out: '', err: e.message }));
  });
}

// Le pregunta al índice de archivos del launcher residente.
//
// Sin residente no hay respuesta, y eso se dice en vez de reconstruir el índice
// aquí: recorrer las unidades fijas cuesta 174 s en esta máquina -- medido con
// `launcher --bench-index`, 1.254.855 entradas -- porque una de ellas es un
// disco mecánico. Contra el índice ya hecho, la misma búsqueda son 21 ms.
//
// Se abre como ARCHIVO (`fs.openSync`) y no con `net.connect`. La capa de
// tuberías de Node devolvía `read EPIPE` siempre, con la respuesta ya escrita
// del lado de Rust: un cliente .NET contra ese mismo servidor, en el mismo
// instante, leía las doce líneas sin queja. El servidor es síncrono y
// bloqueante, así que leerlo como un archivo es además lo que le corresponde.
function preguntarArchivos(consulta) {
  let fd;
  try {
    fd = fs.openSync(TUBERIA, 'r+');
    fs.writeSync(fd, `${consulta}\n`);
    const buf = Buffer.alloc(64 * 1024);
    const n = fs.readSync(fd, buf, 0, buf.length, null);
    return buf.subarray(0, n).toString();
  } catch {
    // No hay launcher residente, o está recién arrancado y todavía no ha
    // abierto la tubería.
    return null;
  } finally {
    if (fd !== undefined) {
      try {
        fs.closeSync(fd);
      } catch {
        /* ya cerrado por el otro lado */
      }
    }
  }
}

// Intenciones que el índice NO puede resolver por sí solo.
//
// El emparejador del launcher es tipográfico: encuentra "resolve" dentro de
// "DaVinci Resolve", pero "editar video" no comparte una sola letra en orden con
// ese nombre, así que devuelve nada -- o, peor, la mejor basura disponible. Esto
// es el puente, y solo debe crecer con casos que se hayan visto fallar de
// verdad; para todo lo demás el índice ya vale y una copia aquí envejecería.
//
// La clave se compara sobre la petición COMPLETA en minúsculas, por inclusión.
const ALIAS = [
  [['editar video', 'editor de video', 'edición de video', 'edicion de video', 'montar video'], 'DaVinci Resolve'],
  [['dibujar', 'pintar', 'ilustrar'], 'CLIP STUDIO'],
  [['grabar pantalla', 'streamear', 'transmitir'], 'OBS Studio (64bit)'],
  [['modelar 3d', 'modelado 3d'], 'Blender'],
  [['navegador', 'internet'], 'Firefox Developer Edition'],
  [['hoja de cálculo', 'hoja de calculo'], 'Excel'],
  [['jugar lol', 'jugar league'], 'League of Legends'],
  [['terminal', 'consola'], 'WezTerm'],
];

function leerAprendidos() {
  try {
    return JSON.parse(fs.readFileSync(APRENDIDOS, 'utf8'));
  } catch {
    // Sin archivo todavía, o editado a mano y roto. Ninguna de las dos cosas
    // debe tumbar una orden: se sigue con los alias del código.
    return {};
  }
}

function porAlias(texto) {
  const t = String(texto).toLowerCase();
  // Lo aprendido gana: si algo se corrigió a mano, esa corrección manda sobre
  // la suposición que traía el código.
  for (const [frase, destino] of Object.entries(leerAprendidos())) {
    if (t.includes(frase.toLowerCase())) return destino;
  }
  for (const [frases, destino] of ALIAS) {
    if (frases.some((f) => t.includes(f))) return destino;
  }
  return null;
}

function aprender(frase, destino) {
  const m = leerAprendidos();
  m[frase.toLowerCase()] = destino;
  fs.writeFileSync(APRENDIDOS, `${JSON.stringify(m, null, 2)}\n`);
  console.log(`aprendido: "${frase}" -> ${destino}`);
}

// Una ruta absoluta de Windows, para distinguirla del nombre de una entrada del
// índice. Es lo que permite que un alias apunte a algo que el índice no ve -- un
// .exe suelto, un archivo de datos -- cuando se busca el path a mano.
function esRuta(s) {
  return /^[a-zA-Z]:[\\/]/.test(s) || s.startsWith('\\\\');
}

const HERRAMIENTAS = {
  abrir: {
    esquema: {
      type: 'function',
      function: {
        name: 'abrir',
        description:
          'Abre una aplicación instalada o ejecuta una acción del sistema (bloquear la sesión, ' +
          'apagar, configuración de sonido, papelera...). Usa el nombre tal y como lo diría una persona.',
        parameters: {
          type: 'object',
          properties: { nombre: { type: 'string', description: 'Nombre de la aplicación o de la acción.' } },
          required: ['nombre'],
        },
      },
    },
    async correr({ nombre }) {
      const destino = porAlias(nombre) ?? nombre;
      if (esRuta(destino)) {
        if (!fs.existsSync(destino)) return `el alias de "${nombre}" apunta a ${destino}, que ya no existe`;
        // FileProtocolHandler abre igual un .exe que un .pdf: decide el shell,
        // que es lo mismo que hace el launcher con sus entradas.
        const r = await correr('rundll32.exe', ['url.dll,FileProtocolHandler', destino]);
        return r.code === 0 ? `abierto: ${destino}` : `no pude abrir ${destino}`;
      }
      const r = await correr(LAUNCHER, ['--abrir', String(destino)]);
      if (r.code !== 0) return `no encontré nada que se llame "${nombre}"`;
      return `abierto: ${r.out}`;
    },
  },

  buscar_app: {
    esquema: {
      type: 'function',
      function: {
        name: 'buscar_app',
        description:
          'Lista qué aplicaciones o acciones coinciden con un nombre, SIN abrir ninguna. ' +
          'Úsalo cuando no estés seguro de cómo se llama algo, antes de abrirlo.',
        parameters: {
          type: 'object',
          properties: { nombre: { type: 'string', description: 'Texto a buscar en el índice.' } },
          required: ['nombre'],
        },
      },
    },
    async correr({ nombre }) {
      const destino = porAlias(nombre) ?? nombre;
      const r = await correr(LAUNCHER, ['--buscar', String(destino)]);
      if (r.code !== 0 || !r.out) return `sin coincidencias para "${nombre}"`;
      // La puntuación es ruido para el modelo; solo estorba al elegir.
      return r.out
        .split('\n')
        .map((l) => l.split('\t').pop())
        .join('\n');
    },
  },

  buscar_archivo: {
    esquema: {
      type: 'function',
      function: {
        name: 'buscar_archivo',
        description:
          'Busca archivos y carpetas por nombre en todo el equipo. Devuelve rutas completas. ' +
          'Úsalo cuando pidan un documento, una carpeta, una foto o un archivo concreto, no un programa.',
        parameters: {
          type: 'object',
          properties: { nombre: { type: 'string', description: 'Parte del nombre del archivo o carpeta.' } },
          required: ['nombre'],
        },
      },
    },
    async correr({ nombre }) {
      const r = preguntarArchivos(String(nombre));
      if (r === null) return 'el índice de archivos no responde (¿está el launcher arrancado?)';
      const lineas = r.split('\n').filter(Boolean);
      if (!lineas.length) return `no hay ningún archivo que se llame "${nombre}"`;
      return lineas.join('\n');
    },
  },

  buscar_web: {
    esquema: {
      type: 'function',
      function: {
        name: 'buscar_web',
        description: 'Abre el navegador con una búsqueda en internet.',
        parameters: {
          type: 'object',
          properties: { consulta: { type: 'string', description: 'Qué buscar.' } },
          required: ['consulta'],
        },
      },
    },
    async correr({ consulta }) {
      // La URL se CONSTRUYE aquí y se codifica: el modelo aporta el texto de
      // búsqueda, nunca el destino. Si pudiera devolver la URL entera, podría
      // mandar el navegador a donde quisiera.
      const url = `https://duckduckgo.com/?q=${encodeURIComponent(String(consulta))}`;
      // rundll32 y no `start`, que necesitaría shell.
      const r = await correr('rundll32.exe', ['url.dll,FileProtocolHandler', url]);
      return r.code === 0 ? `buscando en el navegador: ${consulta}` : `no pude abrir el navegador: ${r.err}`;
    },
  },
};

const SISTEMA = `Eres las manos de un asistente en un PC con Windows 11 en español.
Traduces lo que pide el usuario a UNA llamada de herramienta.

Reglas:
- Si te piden abrir, arrancar, lanzar o ejecutar un PROGRAMA: usa "abrir".
- Si el nombre del programa es ambiguo o no estás seguro: primero "buscar_app", y luego "abrir" con el nombre exacto de la lista.
- Si te piden un ARCHIVO, una carpeta, un documento o una foto: usa "buscar_archivo", y si quieren abrirlo, pasa a "abrir" la ruta completa que te devuelva.
- Si te piden buscar información, noticias, precios o cualquier cosa de internet: usa "buscar_web".
- NUNCA inventes un nombre de programa para probar suerte. Solo puedes pasar a "abrir" un nombre que el usuario haya dicho, o uno que hayas visto en la lista que devolvió "buscar_app" o "buscar_archivo".
- Si no encuentras algo tras un par de intentos, responde que no lo encuentras. Eso es una respuesta correcta.
- Cuando la acción ya esté hecha, responde en una frase corta en español. No repitas la llamada.`;

async function pedir(mensajes) {
  const res = await fetch(`${SERVIDOR}/v1/chat/completions`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      messages: mensajes,
      tools: Object.values(HERRAMIENTAS).map((h) => h.esquema),
      tool_choice: 'auto',
      temperature: 0.2,
      max_tokens: 512,
      // Qwen piensa por defecto y aquí el pensamiento no aporta: son órdenes de
      // una frase, y el presupuesto de tokens se iba en deliberar antes de
      // llamar a la herramienta.
      chat_template_kwargs: { enable_thinking: false },
    }),
  });
  if (!res.ok) throw new Error(`servidor ${res.status}: ${await res.text()}`);
  return (await res.json()).choices[0].message;
}

async function main() {
  const argv = process.argv.slice(2);

  if (argv[0] === '--aprender') {
    if (argv.length < 3) {
      console.error('uso: node rice-manos.mjs --aprender "<frase>" "<nombre o ruta>"');
      process.exit(2);
    }
    aprender(argv[1], argv.slice(2).join(' '));
    return;
  }
  if (argv[0] === '--alias') {
    const m = leerAprendidos();
    const n = Object.keys(m).length;
    console.log(n ? JSON.stringify(m, null, 2) : 'todavía no se ha aprendido nada');
    return;
  }

  const seco = argv[0] === '--seco';
  const peticion = (seco ? argv.slice(1) : argv).join(' ');
  if (!peticion) {
    console.error('uso: node rice-manos.mjs [--seco] "<lo que quieres>"');
    process.exit(2);
  }

  // Si la petición entera cae en un alias, se le dice al modelo cuál es el
  // nombre real. Recorta la frase a su antojo al rellenar el argumento y a
  // veces se deja fuera justo las palabras que disparan el alias.
  const pista = porAlias(peticion);
  const mensajes = [
    { role: 'system', content: SISTEMA },
    {
      role: 'user',
      content: pista ? `${peticion}\n\n(en este PC eso es "${pista}")` : peticion,
    },
  ];

  for (let vuelta = 0; vuelta < MAX_VUELTAS; vuelta++) {
    const m = await pedir(mensajes);
    mensajes.push(m);
    const llamadas = m.tool_calls ?? [];
    if (!llamadas.length) {
      console.log(m.content?.trim() || '(sin respuesta)');
      return;
    }
    for (const c of llamadas) {
      const h = HERRAMIENTAS[c.function.name];
      let resultado;
      if (!h) {
        resultado = `no existe la herramienta "${c.function.name}"`;
      } else {
        let args = {};
        try {
          args = JSON.parse(c.function.arguments || '{}');
        } catch {
          resultado = 'argumentos ilegibles';
        }
        if (resultado === undefined) {
          console.log(`> ${c.function.name}(${JSON.stringify(args)})`);
          resultado = seco ? '(simulacro: no se ejecutó)' : await h.correr(args);
        }
      }
      console.log(`  ${resultado}`);
      mensajes.push({ role: 'tool', tool_call_id: c.id, content: resultado });
    }
  }
  console.error(`se agotaron las ${MAX_VUELTAS} vueltas sin terminar`);
  process.exit(1);
}

main().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
