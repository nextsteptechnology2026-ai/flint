//! El archivo de configuración: `~/.config/flint/config.toml`.
//!
//! Es lo que separa "preferencia" de "recompilado". Hasta acá el ancho de
//! tabulación vivía en el tema, el perfil de teclado solo se elegía por
//! bandera y el comando del servidor de lenguaje estaba escrito en el
//! código; las tres cosas se deciden ahora en este archivo.
//!
//! El criterio de carga es el mismo que el de `theme.toml`: todo es
//! opcional, lo que falta se queda en su valor por defecto, y un valor mal
//! escrito se avisa y se descarta *solo él* — nunca aborta la carga ni deja
//! el resto del archivo sin aplicar.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::clipboard;
use crate::editor::DEFAULT_TAB_WIDTH;
use crate::keymap::{self, Action, KeyChord};

/// Igual que en el tema: más que esto no es una preferencia de sangría, es
/// una línea entera por cada nivel.
const MAX_TAB_WIDTH: usize = 16;

/// Lo que se puede decidir por archivo. Cada buffer resuelve su propia copia
/// al abrirse, aplicando encima de la base los bloques `[files."…"]` cuyo
/// patrón le calce.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Options {
    /// Columnas que ocupa un tabulador al dibujarse.
    pub tab_width: usize,
    /// Si al indentar se insertan espacios en vez de un `\t`.
    pub indent_with_spaces: bool,
    /// Ajuste de línea al abrir.
    pub wrap: bool,
    /// Sacar los espacios del final de cada línea al guardar.
    pub trim_trailing_whitespace: bool,
    /// Cerrar solo los paréntesis, corchetes, llaves y comillas.
    pub auto_close_brackets: bool,
    /// Pedirle al servidor de lenguaje que formatee el archivo antes de
    /// guardarlo.
    pub format_on_save: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            tab_width: DEFAULT_TAB_WIDTH,
            indent_with_spaces: false,
            wrap: false,
            trim_trailing_whitespace: false,
            auto_close_brackets: true,
            format_on_save: false,
        }
    }
}

/// Un remapeo del archivo de configuración. `action: None` significa
/// "desatar esta tecla": queda sin hacer nada en vez de volver al default.
pub struct KeyBinding {
    pub chord: KeyChord,
    pub action: Option<Action>,
    /// true si es para la capa modal NORMAL (`[modal_keys]`), false para la
    /// capa directa e INSERT (`[keys]`).
    pub modal: bool,
}

#[derive(Default)]
pub struct Config {
    /// Perfil de teclado por defecto; `--profile` en la línea de comandos
    /// sigue mandando por encima de esto.
    pub profile: Option<String>,
    /// Tema por defecto; `--theme` manda por encima.
    pub theme: Option<PathBuf>,
    base: RawOptions,
    /// Patrón → opciones, ordenados de patrón más corto a más largo, que es
    /// el orden en que se aplican (ver `options_for`).
    per_file: Vec<(String, RawOptions)>,
    /// A dónde va lo que se copia. Es una decisión de la máquina entera, no
    /// de cada archivo, así que se lee solo de `[options]`.
    pub clipboard: clipboard::Mode,
    /// Identificador de lenguaje LSP → comando y argumentos del servidor.
    pub lsp: BTreeMap<String, Vec<String>>,
    pub keys: Vec<KeyBinding>,
}

/// El archivo como lo entiende serde: todo opcional.
#[derive(Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    options: RawOptions,
    #[serde(default)]
    files: BTreeMap<String, RawOptions>,
    #[serde(default)]
    lsp: BTreeMap<String, RawCommand>,
    #[serde(default)]
    keys: BTreeMap<String, String>,
    #[serde(default)]
    modal_keys: BTreeMap<String, String>,
}

#[derive(Deserialize, Default, Clone)]
struct RawOptions {
    profile: Option<String>,
    theme: Option<String>,
    clipboard: Option<String>,
    tab_width: Option<usize>,
    indent_with_spaces: Option<bool>,
    wrap: Option<bool>,
    trim_trailing_whitespace: Option<bool>,
    auto_close_brackets: Option<bool>,
    format_on_save: Option<bool>,
}

/// El comando de un servidor se puede escribir como `"pylsp"` o como
/// `["pylsp", "--check-parent-process"]` — la segunda forma es la única que
/// permite pasarle argumentos, y la primera es la que se quiere escribir el
/// noventa por ciento de las veces.
#[derive(Deserialize, Clone)]
#[serde(untagged)]
enum RawCommand {
    Simple(String),
    WithArgs(Vec<String>),
}

impl RawCommand {
    fn into_parts(self) -> Vec<String> {
        match self {
            RawCommand::Simple(s) => vec![s],
            RawCommand::WithArgs(v) => v,
        }
    }
}

impl Config {
    /// Ruta por defecto: `~/.config/flint/config.toml`, al lado del tema.
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".config/flint/config.toml"))
    }

    /// Carga la configuración desde `path`. Devuelve lo que se pudo entender
    /// y la lista de avisos de lo que no.
    pub fn load(path: &Path) -> (Config, Vec<String>) {
        let mut warnings = Vec::new();
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("No se pudo leer {}: {e}", path.display()));
                return (Config::default(), warnings);
            }
        };
        let raw: RawConfig = match toml::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                warnings.push(format!("{}: {e}", path.display()));
                return (Config::default(), warnings);
            }
        };

        let mut cfg = Config {
            profile: raw.options.profile.clone(),
            theme: raw.options.theme.clone().map(PathBuf::from),
            base: raw.options.clone(),
            ..Config::default()
        };
        check_tab_width("options", &cfg.base, &mut warnings);

        if let Some(nombre) = &raw.options.clipboard {
            match clipboard::Mode::from_name(nombre) {
                Some(m) => cfg.clipboard = m,
                None => warnings.push(format!(
                    "options.clipboard = \"{nombre}\" desconocido (usá auto, system, terminal o internal)"
                )),
            }
        }

        // De patrón más corto a más largo: cuando dos patrones alcanzan al
        // mismo archivo gana el más largo, que es el más específico de los
        // dos ("*.rs" cede ante "src/*.rs"). A igual largo, orden alfabético,
        // para que el resultado no dependa de cómo quedó escrito el archivo.
        let mut per_file: Vec<(String, RawOptions)> = raw.files.into_iter().collect();
        per_file.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
        for (pat, opts) in &per_file {
            check_tab_width(&format!("files.\"{pat}\""), opts, &mut warnings);
        }
        cfg.per_file = per_file;

        for (lang, cmd) in raw.lsp {
            let parts = cmd.into_parts();
            if parts.is_empty() || parts[0].trim().is_empty() {
                warnings.push(format!("lsp.{lang}: el comando está vacío"));
                continue;
            }
            cfg.lsp.insert(lang.to_ascii_lowercase(), parts);
        }

        parse_keys(&raw.keys, false, &mut cfg.keys, &mut warnings);
        parse_keys(&raw.modal_keys, true, &mut cfg.keys, &mut warnings);

        (cfg, warnings)
    }

    /// Las opciones que le tocan a `path`: la base del archivo, con los
    /// bloques `[files."…"]` que le calcen aplicados encima. `fallback` es de
    /// dónde salen los valores que la configuración no menciona (el ancho de
    /// tabulación del tema, por ejemplo).
    pub fn options_for(&self, path: Option<&Path>, fallback: &Options) -> Options {
        let mut o = *fallback;
        apply(&mut o, &self.base);
        if let Some(path) = path {
            for (pat, opts) in &self.per_file {
                if glob_matches(pat, path) {
                    apply(&mut o, opts);
                }
            }
        }
        o
    }

    /// El comando del servidor de lenguaje para este identificador LSP, si
    /// la configuración define uno.
    pub fn lsp_command(&self, lang_id: &str) -> Option<&[String]> {
        self.lsp.get(lang_id).map(Vec::as_slice)
    }
}

fn apply(o: &mut Options, raw: &RawOptions) {
    if let Some(w) = raw.tab_width
        && (1..=MAX_TAB_WIDTH).contains(&w)
    {
        o.tab_width = w;
    }
    if let Some(b) = raw.indent_with_spaces {
        o.indent_with_spaces = b;
    }
    if let Some(b) = raw.wrap {
        o.wrap = b;
    }
    if let Some(b) = raw.trim_trailing_whitespace {
        o.trim_trailing_whitespace = b;
    }
    if let Some(b) = raw.auto_close_brackets {
        o.auto_close_brackets = b;
    }
    if let Some(b) = raw.format_on_save {
        o.format_on_save = b;
    }
}

fn check_tab_width(seccion: &str, raw: &RawOptions, warnings: &mut Vec<String>) {
    if let Some(w) = raw.tab_width
        && !(1..=MAX_TAB_WIDTH).contains(&w)
    {
        warnings.push(format!(
            "{seccion}.tab_width = {w} fuera de rango (usá un número entre 1 y {MAX_TAB_WIDTH})"
        ));
    }
}

fn parse_keys(
    raw: &BTreeMap<String, String>,
    modal: bool,
    out: &mut Vec<KeyBinding>,
    warnings: &mut Vec<String>,
) {
    let seccion = if modal { "modal_keys" } else { "keys" };
    for (tecla, nombre) in raw {
        let chord = match keymap::parse_chord(tecla) {
            Ok(c) => c,
            Err(e) => {
                warnings.push(format!("{seccion}: {e}"));
                continue;
            }
        };
        // "none" es desatar la tecla a propósito, y se distingue de un
        // nombre mal escrito: lo primero es una decisión, lo segundo un error
        // que hay que avisar.
        let action = if nombre.trim().eq_ignore_ascii_case("none") {
            None
        } else {
            match Action::from_name(nombre) {
                Some(a) => Some(a),
                None => {
                    warnings.push(format!(
                        "{seccion}.\"{tecla}\": no existe la acción \"{nombre}\" (ver la lista con --actions)"
                    ));
                    continue;
                }
            }
        };
        out.push(KeyBinding {
            chord,
            action,
            modal,
        });
    }
}

/// Coincidencia de patrón al estilo shell, con `*` (cualquier cosa, incluso
/// nada) y `?` (exactamente un carácter). Un patrón sin `/` se compara solo
/// contra el nombre del archivo (`*.rs` alcanza a `src/main.rs`); uno con
/// `/` se compara contra la ruta entera tal como se escribió al abrir.
fn glob_matches(pattern: &str, path: &Path) -> bool {
    let objetivo = if pattern.contains('/') {
        path.to_string_lossy().to_string()
    } else {
        match path.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => return false,
        }
    };
    wildcard(
        &pattern.chars().collect::<Vec<char>>(),
        &objetivo.chars().collect::<Vec<char>>(),
    )
}

/// El algoritmo clásico con vuelta atrás sobre el último `*`: recorre los dos
/// a la vez y, cuando algo no calza, retrocede a probar que ese `*` se coma
/// un carácter más. Es lineal en la práctica y no recursivo, así que un
/// patrón patológico no puede desbordar la pila.
fn wildcard(pat: &[char], txt: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut estrella: Option<usize> = None;
    let mut vuelta = 0usize;
    while t < txt.len() {
        if p < pat.len() && (pat[p] == '?' || pat[p] == txt[t]) {
            p += 1;
            t += 1;
        } else if p < pat.len() && pat[p] == '*' {
            estrella = Some(p);
            vuelta = t;
            p += 1;
        } else if let Some(e) = estrella {
            p = e + 1;
            vuelta += 1;
            t = vuelta;
        } else {
            return false;
        }
    }
    // Lo que sobre del patrón solo puede ser una cola de estrellas.
    pat[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opciones(toml_src: &str, archivo: &str) -> Options {
        let dir = std::env::temp_dir().join(format!("flint-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join(format!("{}.toml", archivo.replace(['/', '.'], "_")));
        std::fs::write(&ruta, toml_src).unwrap();
        let (cfg, avisos) = Config::load(&ruta);
        assert!(avisos.is_empty(), "avisos inesperados: {avisos:?}");
        cfg.options_for(Some(Path::new(archivo)), &Options::default())
    }

    #[test]
    fn formatear_al_guardar_viene_apagado_y_se_prende_por_archivo() {
        let src = "[files.\"*.rs\"]\nformat_on_save = true\n";
        assert!(opciones(src, "src/main.rs").format_on_save);
        assert!(!opciones(src, "script.py").format_on_save);
        assert!(!Options::default().format_on_save);
    }

    #[test]
    fn glob_por_nombre_y_por_ruta() {
        assert!(glob_matches("*.rs", Path::new("src/main.rs")));
        assert!(!glob_matches("*.rs", Path::new("src/main.py")));
        assert!(glob_matches("src/*.rs", Path::new("src/main.rs")));
        // Con `/` en el patrón la comparación es contra la ruta entera, así
        // que el mismo archivo abierto por otro camino no calza.
        assert!(!glob_matches("src/*.rs", Path::new("main.rs")));
        assert!(glob_matches("?.txt", Path::new("a.txt")));
        assert!(!glob_matches("?.txt", Path::new("ab.txt")));
        assert!(glob_matches("*", Path::new("cualquiera")));
    }

    #[test]
    fn el_patron_mas_largo_gana() {
        let o = opciones(
            r#"
[files."*.rs"]
tab_width = 8
[files."src/*.rs"]
tab_width = 2
"#,
            "src/main.rs",
        );
        assert_eq!(o.tab_width, 2);
    }

    #[test]
    fn las_opciones_por_archivo_pisan_la_base() {
        let src = r#"
[options]
indent_with_spaces = false
tab_width = 8
[files."*.py"]
indent_with_spaces = true
tab_width = 4
"#;
        let py = opciones(src, "script.py");
        assert!(py.indent_with_spaces);
        assert_eq!(py.tab_width, 4);
        let rs = opciones(src, "main.rs");
        assert!(!rs.indent_with_spaces);
        assert_eq!(rs.tab_width, 8);
    }

    #[test]
    fn un_valor_invalido_avisa_y_no_se_aplica() {
        let dir = std::env::temp_dir().join(format!("flint-cfg-inv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("config.toml");
        std::fs::write(
            &ruta,
            "[options]\ntab_width = 99\nwrap = true\n[keys]\n\"ctrl+j\" = \"no_existe\"\n",
        )
        .unwrap();
        let (cfg, avisos) = Config::load(&ruta);
        assert_eq!(avisos.len(), 2, "avisos: {avisos:?}");
        let o = cfg.options_for(None, &Options::default());
        // El ancho fuera de rango se descartó, pero el resto del archivo se
        // aplicó igual.
        assert_eq!(o.tab_width, DEFAULT_TAB_WIDTH);
        assert!(o.wrap);
        assert!(cfg.keys.is_empty());
    }

    #[test]
    fn las_teclas_se_leen_en_las_dos_capas() {
        let dir = std::env::temp_dir().join(format!("flint-cfg-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("config.toml");
        std::fs::write(
            &ruta,
            "[keys]\n\"ctrl+j\" = \"newline\"\n\"ctrl+w\" = \"none\"\n[modal_keys]\n\"g\" = \"move_home\"\n",
        )
        .unwrap();
        let (cfg, avisos) = Config::load(&ruta);
        assert!(avisos.is_empty(), "avisos: {avisos:?}");
        assert_eq!(cfg.keys.len(), 3);
        let desatada = cfg.keys.iter().find(|k| k.action.is_none()).unwrap();
        assert!(!desatada.modal);
        assert!(cfg.keys.iter().any(|k| k.modal && k.action == Some(Action::MoveHome(false))));
    }

    /// El archivo de ejemplo del repositorio tiene que cargar limpio: es lo
    /// primero que alguien copia a `~/.config/flint/config.toml`, así que un
    /// aviso ahí es un aviso en la máquina de todos.
    #[test]
    fn el_ejemplo_del_repositorio_carga_sin_avisos() {
        let ruta = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
        let (cfg, avisos) = Config::load(&ruta);
        assert!(avisos.is_empty(), "avisos: {avisos:?}");
        assert_eq!(cfg.profile.as_deref(), Some("flint"));
        assert_eq!(cfg.lsp_command("rust"), Some(["rust-analyzer".to_string()].as_slice()));
        // Las opciones por archivo del ejemplo tienen que hacer algo.
        let py = cfg.options_for(Some(Path::new("script.py")), &Options::default());
        assert!(py.indent_with_spaces);
        assert!(!cfg.keys.is_empty());
    }

    #[test]
    fn el_comando_lsp_acepta_las_dos_formas() {
        let dir = std::env::temp_dir().join(format!("flint-cfg-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("config.toml");
        std::fs::write(
            &ruta,
            "[lsp]\nrust = \"rust-analyzer\"\npython = [\"pylsp\", \"-v\"]\n",
        )
        .unwrap();
        let (cfg, avisos) = Config::load(&ruta);
        assert!(avisos.is_empty(), "avisos: {avisos:?}");
        assert_eq!(cfg.lsp_command("rust"), Some(["rust-analyzer".to_string()].as_slice()));
        assert_eq!(
            cfg.lsp_command("python"),
            Some(["pylsp".to_string(), "-v".to_string()].as_slice())
        );
    }
}
