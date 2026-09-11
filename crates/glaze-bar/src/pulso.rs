//! Cliente de Pulso: las barras de hábitos cotidianos.
//!
//! Habla con `pulso-api` en agapornis. El servidor resuelve lo dificil -- las
//! pausas, los marcados anulados, cual cuenta como ultimo -- y manda `vence`,
//! un instante absoluto. La barra solo divide, y por eso puede hacerlo en cada
//! repintado: nada de ticks, nada de contadores que decrementar.
//!
//! Esa division esta repetida a los dos lados a proposito. Lo que no se repite
//! es el criterio; si cambiara, cambia en el servidor y los tres clientes lo
//! heredan dentro de `vence`.
//!
//! La URL y el token viven en `~/.config/rice-secrets.json`, no en `rice.json`:
//! el repo del rice es PÚBLICO, y ahí no va ni el token ni el nombre del
//! servidor.
//!
//! Sin conexión no se pierde nada. Un marcado que no sale se guarda en
//! `~/.config/pulso-cola.json` y se reintenta en el siguiente sondeo. Es seguro
//! porque el modelo solo AÑADE filas: reenviarlas tarde, o dos veces desde
//! sitios distintos, da el mismo resultado.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Un hábito tal y como lo manda el servidor. Solo los campos que la barra
/// pinta; serde ignora el resto sin quejarse.
#[derive(Debug, Clone, Deserialize)]
pub struct Habito {
    pub id: String,
    pub nombre: String,
    #[serde(default)]
    pub icono: Option<String>,
    /// La foto que mandó el servidor al responder. NO se pinta directamente:
    /// ver `barra_ahora`.
    pub barra: f32,
    /// "4 h", "45 min", "vencido", tal y como estaba al responder.
    pub falta: String,
    /// Cuándo vence, en absoluto. Es EL dato: con esto la barra se calcula en
    /// cualquier instante sin volver a preguntar.
    pub vence: String,
    /// La escala de la barra, en segundos.
    #[serde(default)]
    pub cada_segundos: i64,
    #[serde(default)]
    pub en_pausa: bool,
    #[serde(default)]
    pub proximo_aviso: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Respuesta {
    habitos: Vec<Habito>,
}

/// Un marcado que no se pudo entregar.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Pendiente {
    habito_id: String,
    /// Cuándo se marcó DE VERDAD, no cuándo se logró enviar. Por eso la cola no
    /// falsea la hora: al reenviarse mañana, sigue diciendo que fue hoy.
    cuando: String,
}

fn config_path(nombre: &str) -> Option<PathBuf> {
    std::env::var("USERPROFILE").ok().map(|h| PathBuf::from(h).join(".config").join(nombre))
}

/// `(url, token)` de `rice-secrets.json`, o `None` si no está configurado --
/// que es el caso normal hasta que despliegues el servidor.
pub fn config() -> Option<(String, String)> {
    let p = config_path("rice-secrets.json")?;
    let txt = std::fs::read_to_string(p).ok()?;
    let j: serde_json::Value = serde_json::from_str(&txt).ok()?;
    let url = j.get("pulso_url")?.as_str()?.trim_end_matches('/').to_string();
    let token = j.get("pulso_token")?.as_str()?.to_string();
    if url.is_empty() || token.is_empty() {
        return None;
    }
    Some((url, token))
}

fn cola_path() -> Option<PathBuf> {
    config_path("pulso-cola.json")
}

fn leer_cola() -> Vec<Pendiente> {
    cola_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn escribir_cola(v: &[Pendiente]) {
    let Some(p) = cola_path() else { return };
    if v.is_empty() {
        let _ = std::fs::remove_file(p);
        return;
    }
    if let Ok(t) = serde_json::to_string_pretty(v) {
        let _ = std::fs::write(p, t);
    }
}

fn enviar_marca(url: &str, token: &str, p: &Pendiente) -> Result<(), String> {
    ureq::post(&format!("{url}/api/hecho"))
        .set("authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(8))
        .send_json(ureq::json!({
            "habito_id": p.habito_id,
            "cuando": p.cuando,
            "origen": "pc",
        }))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Marca un hábito. Si la red falla, va a la cola en vez de perderse.
pub fn marcar(habito_id: &str) -> Result<(), String> {
    let p = Pendiente {
        habito_id: habito_id.to_string(),
        cuando: chrono::Utc::now().to_rfc3339(),
    };
    let Some((url, token)) = config() else {
        return Err("pulso no configurado".into());
    };
    match enviar_marca(&url, &token, &p) {
        Ok(()) => Ok(()),
        Err(e) => {
            let mut cola = leer_cola();
            cola.push(p);
            escribir_cola(&cola);
            Err(format!("{e} (encolado)"))
        }
    }
}

/// Reintenta la cola. Lo que siga fallando se queda para la próxima.
fn vaciar_cola(url: &str, token: &str) {
    let cola = leer_cola();
    if cola.is_empty() {
        return;
    }
    let quedan: Vec<Pendiente> =
        cola.into_iter().filter(|p| enviar_marca(url, token, p).is_err()).collect();
    escribir_cola(&quedan);
}

/// El estado de todos los hábitos. Vacía la cola de paso: si hay red para leer,
/// la hay para escribir.
pub fn estado() -> Result<Vec<Habito>, String> {
    let Some((url, token)) = config() else {
        return Err("pulso no configurado".into());
    };
    vaciar_cola(&url, &token);

    let r = ureq::get(&format!("{url}/api/estado"))
        .set("authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(8))
        .call()
        .map_err(|e| e.to_string())?;
    let cuerpo: Respuesta = r.into_json().map_err(|e| e.to_string())?;
    Ok(cuerpo.habitos)
}

impl Habito {
    /// La barra AHORA, no cuando contestó el servidor.
    ///
    /// Se interpola desde `vence`, que es un instante absoluto. Es la misma
    /// fórmula que tiene `pulso-core` del lado del servidor, dos líneas
    /// repetidas a propósito: lo difícil -- las pausas, los marcados anulados,
    /// qué cuenta como último -- sigue en un solo sitio y sale ya resuelto
    /// dentro de `vence`. Esto solo divide.
    ///
    /// Se recalcula entero cada vez en vez de restarle a un contador. Un
    /// contador acumula deriva y, sobre todo, miente después de suspender o
    /// apagar el equipo -- que es todas las noches.
    pub fn barra_ahora(&self) -> f32 {
        if self.cada_segundos <= 0 {
            return self.barra;
        }
        match chrono::DateTime::parse_from_rfc3339(&self.vence) {
            Ok(v) => {
                let quedan = (v.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
                (quedan as f32 / self.cada_segundos as f32).clamp(0.0, 1.0)
            }
            // Sin fecha legible, la foto del servidor es mejor que nada.
            Err(_) => self.barra,
        }
    }

    /// Cuánto queda, en palabras, recalculado igual que la barra.
    pub fn falta_ahora(&self) -> String {
        let Ok(v) = chrono::DateTime::parse_from_rfc3339(&self.vence) else {
            return self.falta.clone();
        };
        let s = (v.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
        if s <= 0 {
            "vencido".into()
        } else if s < 5400 {
            format!("{} min", s / 60)
        } else if s < 172_800 {
            format!("{} h", s / 3600)
        } else {
            format!("{} d", s / 86_400)
        }
    }
}

/// El más urgente de la lista: el de la barra más baja, saltándose los pausados.
/// Es el único que cabe en la tira; el resto vive en el panel de la isla.
pub fn mas_urgente(v: &[Habito]) -> Option<&Habito> {
    v.iter().filter(|h| !h.en_pausa).min_by(|a, b| {
        a.barra_ahora().partial_cmp(&b.barra_ahora()).unwrap_or(std::cmp::Ordering::Equal)
    })
}
