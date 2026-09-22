//! Buscar texto en todo el proyecto (Ctrl+N).
//!
//! Al abrir el buscador se lee el proyecto una vez a memoria y cada tecla
//! busca sobre esa copia: así se puede buscar mientras se escribe, como en
//! el buscador de archivos, sin volver a tocar el disco por cada letra. Lo
//! que se lee sale del mismo índice que usa Ctrl+T, con los mismos
//! directorios ignorados.

use std::path::Path;

use regex::{Regex, RegexBuilder};

/// Un archivo más grande que esto casi nunca es algo escrito a mano (datos,
/// minificados, logs) y arrastraría cada búsqueda.
const MAX_BYTES_POR_ARCHIVO: u64 = 1024 * 1024;
/// Tope de lo que se carga en total. Con dos copias por archivo (la original
/// y la de minúsculas) son hasta ~64 MB en memoria mientras el buscador está
/// abierto, y se liberan al cerrarlo.
const MAX_BYTES_TOTALES: usize = 32 * 1024 * 1024;
/// Más coincidencias que esto no se pueden recorrer a ojo; mejor pedir que
/// se afine la búsqueda que esperar a juntar cien mil.
pub const MAX_COINCIDENCIAS: usize = 1000;
/// Hasta cuántos caracteres de la línea se muestran en la lista.
const MAX_TEXTO: usize = 160;

pub struct Archivo {
    /// Ruta relativa a la raíz del proyecto, como la muestra el buscador.
    pub ruta: String,
    texto: String,
    /// El mismo texto en minúsculas, para buscar sin distinguir mayúsculas
    /// sin tener que convertir el archivo entero en cada tecla.
    minusculas: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coincidencia {
    /// Índice en la lista de archivos cargados.
    pub archivo: usize,
    /// Línea y columna (en caracteres) donde empieza, contadas desde 0.
    pub linea: usize,
    pub col: usize,
    /// La línea donde está, sin la sangría y recortada.
    pub texto: String,
}

pub struct Resultado {
    pub coincidencias: Vec<Coincidencia>,
    /// Si se cortó en `MAX_COINCIDENCIAS` y quedan más.
    pub recortado: bool,
    pub archivos: usize,
}

/// Un archivo con un byte NUL al principio es binario: un ejecutable, una
/// imagen, un .deb. Es la misma regla que usan git y grep.
fn es_binario(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

impl Archivo {
    pub fn nuevo(ruta: String, texto: String) -> Archivo {
        let minusculas = texto.to_lowercase();
        Archivo { ruta, texto, minusculas }
    }
}

/// Lee a memoria los archivos de `rutas` (relativas a `raiz`). `abierto`
/// devuelve el contenido que tiene el editor para esa ruta, si está abierta:
/// ese manda sobre el del disco, porque lo que se busca es lo que uno ve,
/// cambios sin guardar incluidos. Se saltean los binarios, los que no son
/// UTF-8 y los demasiado grandes. El `bool` dice si se cortó en el tope
/// total antes de leer todo, para poder avisarlo: buscar en medio proyecto
/// sin saberlo es peor que no encontrar.
pub fn cargar(
    raiz: &Path,
    rutas: &[String],
    abierto: impl Fn(&str) -> Option<String>,
) -> (Vec<Archivo>, bool) {
    let mut archivos = Vec::new();
    let mut total = 0usize;
    for ruta in rutas {
        if total >= MAX_BYTES_TOTALES {
            return (archivos, true);
        }
        let texto = match abierto(ruta) {
            Some(t) => t,
            None => {
                let completa = raiz.join(ruta);
                match std::fs::metadata(&completa) {
                    Ok(m) if m.len() <= MAX_BYTES_POR_ARCHIVO => {}
                    _ => continue,
                }
                let Ok(bytes) = std::fs::read(&completa) else { continue };
                if es_binario(&bytes) {
                    continue;
                }
                let Ok(t) = String::from_utf8(bytes) else { continue };
                t
            }
        };
        total += texto.len();
        archivos.push(Archivo::nuevo(ruta.clone(), texto));
    }
    (archivos, false)
}

/// Busca `consulta` como texto literal en todos los archivos. Distingue
/// mayúsculas solo si la consulta tiene alguna (la regla "smartcase" de Vim
/// y ripgrep): `error` encuentra `Error` y `ERROR`, pero `Error` encuentra
/// solo `Error`. Una coincidencia por línea, en el orden de los archivos.
pub fn buscar(archivos: &[Archivo], consulta: &str) -> Resultado {
    let mut coincidencias = Vec::new();
    let mut con_algo = 0usize;
    if consulta.is_empty() {
        return Resultado { coincidencias, recortado: false, archivos: 0 };
    }
    let distinguir = consulta.chars().any(char::is_uppercase);
    let aguja = if distinguir { consulta.to_string() } else { consulta.to_lowercase() };

    for (i, archivo) in archivos.iter().enumerate() {
        let pajar = if distinguir { &archivo.texto } else { &archivo.minusculas };
        let mut hubo = false;
        let mut desde = 0usize;
        // Se cuentan las líneas a medida que se avanza en vez de partir el
        // archivo entero: la mayoría de los archivos no tiene ninguna
        // coincidencia y ahí `find` recorre el texto una sola vez.
        let mut linea = 0usize;
        let mut contado_hasta = 0usize;
        // Las líneas del original avanzan junto con las de la búsqueda, así
        // el archivo se recorre una vez sin importar cuántas coincidencias
        // tenga.
        let mut originales = archivo.texto.lines();
        let mut siguiente_original = 0usize;
        while let Some(rel) = pajar[desde..].find(&aguja) {
            let byte = desde + rel;
            linea += pajar[contado_hasta..byte].matches('\n').count();
            let inicio_linea = pajar[..byte].rfind('\n').map_or(0, |p| p + 1);
            let fin_linea = pajar[byte..].find('\n').map_or(pajar.len(), |p| byte + p);
            let col = pajar[inicio_linea..byte].chars().count();

            // El texto que se muestra sale del original, no de las
            // minúsculas. Las líneas son las mismas en los dos (pasar a
            // minúsculas no toca los saltos), así que se busca por número.
            let original = originales.nth(linea - siguiente_original).unwrap_or("");
            siguiente_original = linea + 1;
            let texto = para_mostrar(original);
            coincidencias.push(Coincidencia { archivo: i, linea, col, texto });
            hubo = true;
            if coincidencias.len() >= MAX_COINCIDENCIAS {
                return Resultado { coincidencias, recortado: true, archivos: con_algo + 1 };
            }
            // Una por línea: se sigue buscando desde la siguiente.
            if fin_linea >= pajar.len() {
                break;
            }
            desde = fin_linea + 1;
            contado_hasta = desde;
            linea += 1;
        }
        if hubo {
            con_algo += 1;
        }
    }
    Resultado { coincidencias, recortado: false, archivos: con_algo }
}

/// La línea como se ve en la lista: sin sangría y recortada. Un tabulador
/// en medio ocuparía un ancho que el popup no controla.
fn para_mostrar(linea: &str) -> String {
    let limpio = linea.trim().replace('\t', " ");
    let mut texto: String = limpio.chars().take(MAX_TEXTO).collect();
    if limpio.chars().count() > MAX_TEXTO {
        texto.push('…');
    }
    texto
}

/// La expresión con la que se busca y se reemplaza: la consulta tal cual si
/// es una regex, o escapada si es texto literal. Misma regla de mayúsculas
/// que `buscar` (lo que va después de una `\` no cuenta: `\S` no es una
/// mayúscula), y `^` y `$` son el principio y el fin de cada línea.
pub fn expresion(consulta: &str, regex: bool) -> Result<Regex, String> {
    let patron = if regex { consulta.to_string() } else { regex::escape(consulta) };
    let mut escapado = false;
    let mut distinguir = false;
    for c in consulta.chars() {
        if escapado && regex {
            escapado = false;
            continue;
        }
        escapado = c == '\\';
        distinguir |= c.is_uppercase();
    }
    RegexBuilder::new(&patron)
        .multi_line(true)
        .case_insensitive(!distinguir)
        .build()
        .map_err(|e| e.to_string().lines().last().unwrap_or("regex inválida").trim().to_string())
}

/// Como `buscar`, pero con una expresión regular. Una coincidencia por
/// línea, igual que la búsqueda literal.
pub fn buscar_con(archivos: &[Archivo], re: &Regex) -> Resultado {
    let mut coincidencias = Vec::new();
    let mut con_algo = 0usize;
    for (i, archivo) in archivos.iter().enumerate() {
        let texto = &archivo.texto;
        let mut linea = 0usize;
        let mut contado_hasta = 0usize;
        let mut ultima = None;
        for m in re.find_iter(texto) {
            linea += texto[contado_hasta..m.start()].matches('\n').count();
            contado_hasta = m.start();
            if ultima == Some(linea) {
                continue;
            }
            ultima = Some(linea);
            let inicio_linea = texto[..m.start()].rfind('\n').map_or(0, |p| p + 1);
            let fin_linea = texto[m.start()..].find('\n').map_or(texto.len(), |p| m.start() + p);
            let col = texto[inicio_linea..m.start()].chars().count();
            let mostrado = para_mostrar(&texto[inicio_linea..fin_linea]);
            coincidencias.push(Coincidencia { archivo: i, linea, col, texto: mostrado });
            if coincidencias.len() >= MAX_COINCIDENCIAS {
                return Resultado { coincidencias, recortado: true, archivos: con_algo + 1 };
            }
        }
        if ultima.is_some() {
            con_algo += 1;
        }
    }
    Resultado { coincidencias, recortado: false, archivos: con_algo }
}

/// Las rutas de los archivos donde `re` aparece al menos una vez. Es lo que
/// toca un reemplazo: todos, no solo los que entraron en la lista.
pub fn rutas_con(archivos: &[Archivo], re: &Regex) -> Vec<String> {
    archivos.iter().filter(|a| re.is_match(&a.texto)).map(|a| a.ruta.clone()).collect()
}

/// Qué cambiar en `texto` para reemplazar cada aparición de `re` por `con`:
/// rangos en bytes y el texto nuevo. Todas las apariciones, no una por
/// línea. Con `regex`, `con` puede usar los grupos (`$1`, `${nombre}`); sin
/// regex va tal cual, aunque tenga un `$`.
pub fn reemplazos(texto: &str, re: &Regex, con: &str, regex: bool) -> Vec<(usize, usize, String)> {
    re.captures_iter(texto)
        .filter_map(|c| {
            let m = c.get(0)?;
            let nuevo = if regex {
                let mut s = String::new();
                c.expand(con, &mut s);
                s
            } else {
                con.to_string()
            };
            Some((m.start(), m.end(), nuevo))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proyecto(archivos: &[(&str, &str)]) -> Vec<Archivo> {
        archivos
            .iter()
            .map(|(r, t)| Archivo::nuevo(r.to_string(), t.to_string()))
            .collect()
    }

    fn donde(r: &Resultado) -> Vec<(usize, usize, usize)> {
        r.coincidencias.iter().map(|c| (c.archivo, c.linea, c.col)).collect()
    }

    #[test]
    fn encuentra_linea_y_columna_en_varios_archivos() {
        let p = proyecto(&[
            ("a.rs", "fn main() {\n    let x = contar();\n}\n"),
            ("b.rs", "// nada\n"),
            ("c.rs", "use contar;\n\nfn f() { contar() }"),
        ]);
        let r = buscar(&p, "contar");
        assert_eq!(donde(&r), vec![(0, 1, 12), (2, 0, 4), (2, 2, 9)]);
        assert_eq!(r.archivos, 2);
        assert!(!r.recortado);
        assert_eq!(r.coincidencias[0].texto, "let x = contar();");
    }

    #[test]
    fn minusculas_no_distinguen_y_mayusculas_si() {
        let p = proyecto(&[("a", "Error\nerror\nERROR\n")]);
        assert_eq!(buscar(&p, "error").coincidencias.len(), 3);
        assert_eq!(donde(&buscar(&p, "Error")), vec![(0, 0, 0)]);
    }

    #[test]
    fn sin_distinguir_tambien_con_tildes() {
        let p = proyecto(&[("a", "// CANCIÓN\nlet canción = 1;\n")]);
        let r = buscar(&p, "canción");
        assert_eq!(donde(&r), vec![(0, 0, 3), (0, 1, 4)]);
        // El texto que se muestra es el original, no el pasado a minúsculas.
        assert_eq!(r.coincidencias[0].texto, "// CANCIÓN");
    }

    #[test]
    fn la_columna_se_cuenta_en_caracteres_no_en_bytes() {
        let p = proyecto(&[("a", "ñandú = objetivo")]);
        assert_eq!(donde(&buscar(&p, "objetivo")), vec![(0, 0, 8)]);
    }

    #[test]
    fn una_coincidencia_por_linea() {
        let p = proyecto(&[("a", "x x x\nx\n")]);
        assert_eq!(donde(&buscar(&p, "x")), vec![(0, 0, 0), (0, 1, 0)]);
    }

    #[test]
    fn la_consulta_vacia_no_encuentra_nada() {
        let p = proyecto(&[("a", "algo")]);
        assert!(buscar(&p, "").coincidencias.is_empty());
    }

    #[test]
    fn se_corta_en_el_tope() {
        let texto = "x\n".repeat(MAX_COINCIDENCIAS + 5);
        let p = proyecto(&[("a", &texto), ("b", "x")]);
        let r = buscar(&p, "x");
        assert_eq!(r.coincidencias.len(), MAX_COINCIDENCIAS);
        assert!(r.recortado);
    }

    #[test]
    fn con_regex_encuentra_por_linea_y_con_la_misma_regla_de_mayusculas() {
        let p = proyecto(&[("a", "fn uno() {}\nfn dos() {}\nlet x = uno();\n"), ("b", "FN tres()")]);
        let re = expresion(r"^fn \w+", true).unwrap();
        assert_eq!(donde(&buscar_con(&p, &re)), vec![(0, 0, 0), (0, 1, 0), (1, 0, 0)]);
        // Con una mayúscula, distingue. La de `\W` no cuenta como mayúscula.
        let re = expresion(r"FN\W", true).unwrap();
        assert_eq!(donde(&buscar_con(&p, &re)), vec![(1, 0, 0)]);
        let re = expresion(r"uno\W", true).unwrap();
        assert_eq!(donde(&buscar_con(&p, &re)), vec![(0, 0, 3), (0, 2, 8)]);
        // Una por línea también con regex.
        let re = expresion("o", true).unwrap();
        assert_eq!(buscar_con(&p, &re).coincidencias.len(), 3);
    }

    #[test]
    fn una_regex_invalida_dice_por_que() {
        let e = expresion("(sin cerrar", true).unwrap_err();
        assert!(e.contains("unclosed") || e.contains("group"), "{e}");
        // En modo literal los paréntesis son texto.
        assert!(expresion("(sin cerrar", false).is_ok());
    }

    #[test]
    fn reemplaza_todas_las_apariciones_con_grupos_solo_en_regex() {
        let re = expresion(r"(\w+)\.len\(\)", true).unwrap();
        let r = reemplazos("a.len() + b.len()", &re, "len($1)", true);
        assert_eq!(r, vec![(0, 7, "len(a)".to_string()), (10, 17, "len(b)".to_string())]);

        // Literal: el `$` va tal cual, y los puntos no son comodines.
        let re = expresion("a.b", false).unwrap();
        let r = reemplazos("a.b axb A.B", &re, "$1", false);
        assert_eq!(r, vec![(0, 3, "$1".to_string()), (8, 11, "$1".to_string())]);
    }

    #[test]
    fn carga_del_disco_salteando_binarios_y_prefiriendo_lo_abierto() {
        let dir = std::env::temp_dir().join(format!("flint-busqueda-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("texto.txt"), "hola en disco").unwrap();
        std::fs::write(dir.join("abierto.txt"), "viejo").unwrap();
        std::fs::write(dir.join("binario.bin"), b"hola\0\x01\x02").unwrap();
        let rutas = vec![
            "abierto.txt".to_string(),
            "binario.bin".to_string(),
            "no-existe.txt".to_string(),
            "texto.txt".to_string(),
        ];
        let (archivos, recortado) = cargar(&dir, &rutas, |r| (r == "abierto.txt").then(|| "hola sin guardar".to_string()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!recortado);

        let nombres: Vec<&str> = archivos.iter().map(|a| a.ruta.as_str()).collect();
        assert_eq!(nombres, vec!["abierto.txt", "texto.txt"]);
        let r = buscar(&archivos, "hola");
        assert_eq!(r.coincidencias[0].texto, "hola sin guardar");
    }
}
