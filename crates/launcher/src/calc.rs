//! La calculadora, al estilo Spotlight: escribes `23*4` y aparece `= 92` como
//! primera fila, Enter la copia al portapapeles. No es una accion que se
//! "lance" -- por eso vive aparte de [`crate::commands::Runner`], que es
//! justo para lo contrario (ver la nota al principio de `commands.rs` sobre
//! no mezclar las dos cosas).
//!
//! Sin dependencia nueva: `+ - * / % ^` y parentesis son de sobra para lo que
//! se escribe en una caja de busqueda, y un evaluador de precedencia de
//! operadores en unas ochenta lineas no pesa lo que trae un crate entero.

/// Lo que se ofrece para la consulta actual, si de verdad parece una cuenta.
pub struct Resultado {
    /// `= 92`, lo que se lee en la fila.
    pub label: String,
    /// El numero en crudo, sin el `= `, que es lo que se copia.
    pub texto_para_copiar: String,
}

pub fn offer(query: &str) -> Option<Resultado> {
    if !parece_expresion(query) {
        return None;
    }
    let valor = evaluar(query).ok()?;
    if !valor.is_finite() {
        return None; // division entre cero, etc. -- silencio, no un error feo
    }
    let texto = formatear(valor);
    Some(Resultado { label: format!("= {texto}"), texto_para_copiar: texto })
}

/// Filtro antes de intentar evaluar nada. Sin esto, escribir "5" (buscando un
/// programa que empiece por 5, un puerto, lo que sea) mostraria "= 5" y
/// robaria la fila superior para un resultado inutil. Solo entra si parece de
/// verdad una cuenta: nada mas que digitos/operadores/parentesis, con algun
/// digito, y no es solo un numero (posiblemente negativo) suelto.
fn parece_expresion(q: &str) -> bool {
    let q = q.trim();
    if q.is_empty() {
        return false;
    }
    if !q.chars().all(|c| c.is_ascii_digit() || " \t.+-*/%^()".contains(c)) {
        return false;
    }
    if !q.chars().any(|c| c.is_ascii_digit()) {
        return false;
    }
    let sin_signo = q.strip_prefix('-').unwrap_or(q).trim_start();
    if sin_signo.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return false; // "5", "-5", "3.14": un numero suelto, no una cuenta
    }
    true
}

// --- el evaluador ----------------------------------------------------------
//
// Descenso recursivo de toda la vida, con la precedencia de siempre:
//   expr  := term (('+' | '-') term)*
//   term  := pot (('*' | '/' | '%') pot)*
//   pot   := unario ('^' unario)*
//   unario:= '-' unario | primario
//   primario := NUMERO | '(' expr ')'

struct Cursor<'a> {
    resto: std::str::Chars<'a>,
    actual: Option<char>,
}

impl<'a> Cursor<'a> {
    fn new(s: &'a str) -> Self {
        let mut resto = s.chars();
        let actual = resto.next();
        Self { resto, actual }
    }
    fn avanzar(&mut self) {
        self.actual = self.resto.next();
    }
    fn saltar_espacios(&mut self) {
        while matches!(self.actual, Some(c) if c.is_whitespace()) {
            self.avanzar();
        }
    }
}

fn evaluar(s: &str) -> Result<f64, ()> {
    let mut c = Cursor::new(s);
    let v = expr(&mut c)?;
    c.saltar_espacios();
    if c.actual.is_some() {
        return Err(()); // sobraron caracteres: "2+2)" o similar
    }
    Ok(v)
}

fn expr(c: &mut Cursor) -> Result<f64, ()> {
    let mut v = term(c)?;
    loop {
        c.saltar_espacios();
        match c.actual {
            Some('+') => {
                c.avanzar();
                v += term(c)?;
            }
            Some('-') => {
                c.avanzar();
                v -= term(c)?;
            }
            _ => return Ok(v),
        }
    }
}

fn term(c: &mut Cursor) -> Result<f64, ()> {
    let mut v = potencia(c)?;
    loop {
        c.saltar_espacios();
        match c.actual {
            Some('*') => {
                c.avanzar();
                v *= potencia(c)?;
            }
            Some('/') => {
                c.avanzar();
                v /= potencia(c)?;
            }
            Some('%') => {
                c.avanzar();
                v %= potencia(c)?;
            }
            _ => return Ok(v),
        }
    }
}

fn potencia(c: &mut Cursor) -> Result<f64, ()> {
    let base = unario(c)?;
    c.saltar_espacios();
    if c.actual == Some('^') {
        c.avanzar();
        let exp = potencia(c)?; // asociativo a la derecha: 2^3^2 = 2^(3^2)
        return Ok(base.powf(exp));
    }
    Ok(base)
}

fn unario(c: &mut Cursor) -> Result<f64, ()> {
    c.saltar_espacios();
    if c.actual == Some('-') {
        c.avanzar();
        return Ok(-unario(c)?);
    }
    primario(c)
}

fn primario(c: &mut Cursor) -> Result<f64, ()> {
    c.saltar_espacios();
    if c.actual == Some('(') {
        c.avanzar();
        let v = expr(c)?;
        c.saltar_espacios();
        if c.actual != Some(')') {
            return Err(());
        }
        c.avanzar();
        return Ok(v);
    }
    let mut num = String::new();
    while matches!(c.actual, Some(ch) if ch.is_ascii_digit() || ch == '.') {
        num.push(c.actual.unwrap());
        c.avanzar();
    }
    if num.is_empty() {
        return Err(());
    }
    num.parse::<f64>().map_err(|_| ())
}

/// Entero cuando el resultado lo es de verdad (o lo bastante cerca: el ruido
/// de coma flotante de algo como 0.1+0.2 no debe ensenarse), decimales
/// recortados el resto de las veces.
fn formatear(x: f64) -> String {
    let r = x.round();
    if (x - r).abs() < 1e-9 * x.abs().max(1.0) && r.abs() < 1e15 {
        return format!("{}", r as i64);
    }
    let s = format!("{x:.10}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(s: &str) -> f64 {
        evaluar(s).unwrap()
    }

    #[test]
    fn aritmetica_basica() {
        assert_eq!(calc("2+2"), 4.0);
        assert_eq!(calc("10-3"), 7.0);
        assert_eq!(calc("6*7"), 42.0);
        assert_eq!(calc("10/4"), 2.5);
        assert_eq!(calc("10%3"), 1.0);
    }

    #[test]
    fn precedencia_y_parentesis() {
        assert_eq!(calc("2+3*4"), 14.0);
        assert_eq!(calc("(2+3)*4"), 20.0);
        assert_eq!(calc("2^10"), 1024.0);
        assert_eq!(calc("2^3^2"), 512.0); // asociativo a la derecha: 2^(3^2)
    }

    #[test]
    fn unario_y_espacios() {
        assert_eq!(calc("-5+3"), -2.0);
        assert_eq!(calc(" 2 + 2 "), 4.0);
        assert_eq!(calc("-(2+3)"), -5.0);
    }

    #[test]
    fn expresion_mal_formada_no_evalua() {
        assert!(evaluar("2+").is_err());
        assert!(evaluar("(2+3").is_err());
        assert!(evaluar("2+2)").is_err());
        assert!(evaluar("").is_err());
    }

    #[test]
    fn filtro_no_secuestra_numeros_sueltos() {
        assert!(!parece_expresion("5"));
        assert!(!parece_expresion("-5"));
        assert!(!parece_expresion("3.14"));
        assert!(!parece_expresion(""));
        assert!(!parece_expresion("firefox"));
        assert!(parece_expresion("2+2"));
        assert!(parece_expresion("3-5"));
        assert!(parece_expresion("(2+3)*4"));
    }

    #[test]
    fn offer_completo() {
        let r = offer("2+2").unwrap();
        assert_eq!(r.label, "= 4");
        assert_eq!(r.texto_para_copiar, "4");

        let r = offer("10/4").unwrap();
        assert_eq!(r.label, "= 2.5");

        assert!(offer("5").is_none()); // numero suelto: no es una cuenta
        assert!(offer("1/0").is_none()); // infinito: silencio, no un error feo
        assert!(offer("2+").is_none()); // incompleta: silencio
    }

    #[test]
    fn ruido_de_coma_flotante_se_redondea() {
        // 0.1 + 0.2 en f64 da 0.30000000000000004; el usuario quiere ver "0.3".
        let r = offer("0.1+0.2").unwrap();
        assert_eq!(r.label, "= 0.3");
    }
}
