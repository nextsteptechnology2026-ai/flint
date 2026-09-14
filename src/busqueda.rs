//! Buscar texto en todo el proyecto (Ctrl+N).
//!
//! Al abrir el buscador se lee el proyecto una vez a memoria y cada tecla
//! busca sobre esa copia: así se puede buscar mientras se escribe, como en
//! el buscador de archivos, sin volver a tocar el disco por cada letra. Lo
//! que se lee sale del mismo índice que usa Ctrl+T, con los mismos
//! directorios ignorados.

use std::path::Path;

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
            // Un tabulador en medio de la lista ocuparía un ancho que el
            // popup no controla.
            let limpio = original.trim().replace('\t', " ");
            let limpio = limpio.as_str();
            let mut texto: String = limpio.chars().take(MAX_TEXTO).collect();
            if limpio.chars().count() > MAX_TEXTO {
                texto.push('…');
            }
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
