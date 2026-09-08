// Las "manos": traduce una petición en español a una acción en el equipo.
//
// El modelo local NO ejecuta nada. Elige una herramienta de una lista cerrada y
// devuelve sus argumentos; este archivo es quien ejecuta, y solo sabe hacer las
// tres cosas de `HERRAMIENTAS`. Un modelo que se invente `rm -rf` no tiene por
// dónde: no existe una herramienta que reciba una orden de shell.
//
// El índice de aplicaciones NO vive aquí. Es el mismo `launcher.exe` de
// Win+Space, llamado con `--abrir` / `--buscar`. Tener una segunda lista sería
// tenerla desactualizada: al instalar cualquier cosa, la caja la encontraría y
// las manos no.
//
//   node rice-manos.mjs "abre el firefox"
//   node rice-manos.mjs --seco "bloquea la sesión"     (dice qué haría, no lo hace)

import { spawn } from 'node:child_process';
import process from 'node:process';

const SERVIDOR = process.env.MANOS_URL ?? 'http://127.0.0.1:8080';
// El árbol VIVO es `~/dev`, no `~/dotfiles`: es de donde salen todos los
// binarios del rice y de donde `sync.ps1` copia hacia el repo. Apuntar al del
// repo funcionaba pero dejaba a las manos usando una compilación que nadie más
// del escritorio usa.
const LAUNCHER = `${process.env.USERPROFILE}\\dev\\target\\release\\launcher.exe`;
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

function porAlias(texto) {
  const t = String(texto).toLowerCase();
  for (const [frases, destino] of ALIAS) {
    if (frases.some((f) => t.includes(f))) return destino;
  }
  return null;
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
- Si te piden abrir, arrancar, lanzar o ejecutar algo: usa "abrir".
- Si el nombre es ambiguo o no estás seguro: primero "buscar_app", y luego "abrir" con el nombre exacto de la lista.
- Si te piden buscar información, noticias, precios o cualquier cosa de internet: usa "buscar_web".
- NUNCA inventes un nombre de programa para probar suerte. Solo puedes pasar a "abrir" un nombre que el usuario haya dicho, o uno que hayas visto en la lista que devolvió "buscar_app".
- Si "buscar_app" no devuelve nada útil tras un par de intentos, responde que no lo encuentras instalado. Eso es una respuesta correcta.
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
      content: pista ? `${peticion}

(en este PC eso es "${pista}")` : peticion,
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
