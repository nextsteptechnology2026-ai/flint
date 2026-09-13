use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, TextProvider,
    Tree,
};

use crate::editor::{Editor, HighlightKind};

/// Todo lo que Flint sabe de un lenguaje, en un solo renglón. Agregar uno es
/// una variante de `Lang` y una fila de `LENGUAJES`, en la misma posición:
/// hay un test que verifica que las dos listas no se desalineen.
pub struct LangDef {
    /// El identificador que usa LSP en `textDocument/didOpen`.
    id: &'static str,
    /// Cómo se muestra en la barra de estado.
    label: &'static str,
    /// Extensiones, sin el punto y en minúscula.
    exts: &'static [&'static str],
    /// Nombres de archivo completos, para los que no tienen extensión.
    filenames: &'static [&'static str],
    /// Otros nombres con los que se lo puede pedir: el de un cerco de
    /// Markdown (```js) o el intérprete de un shebang (`#!/bin/sh`).
    alias: &'static [&'static str],
    line_comment: Option<&'static str>,
    indents_after_colon: bool,
    /// Comando del servidor LSP, solo si arranca sin argumentos sobre stdio.
    /// Los que necesitan banderas (`--stdio`, `start`) van en `[lsp]` del
    /// config.toml, que acepta el comando completo con sus argumentos —
    /// ponerlos acá a medias haría que Flint intente arrancarlos mal.
    lsp_command: Option<&'static str>,
    /// La gramática y su consulta de resaltado. Es una función porque
    /// `LanguageFn::into()` no se puede evaluar en una constante, y devuelve
    /// `String` porque algunas consultas son la suma de dos: la de
    /// TypeScript es un delta que se apoya en la de JavaScript.
    grammar: fn() -> (Language, String),
    /// Consulta de inyecciones: qué tramos de un archivo de este lenguaje se
    /// analizan con la gramática de otro. Es lo que hace que el `<style>` de
    /// un HTML se vea como CSS y el cerco de código de un Markdown como el
    /// lenguaje que declara. Vacía si el lenguaje no delega en nadie.
    inyecciones: &'static str,
}

/// Lenguajes con resaltado real vía tree-sitter. Lo que no está acá se
/// muestra como texto plano — sigue siendo editable, solo sin color.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Rust,
    Python,
    Json,
    Toml,
    Markdown,
    Bash,
    C,
    Cpp,
    Css,
    Go,
    Html,
    Java,
    JavaScript,
    Lua,
    TypeScript,
    Tsx,
    Yaml,
}

/// En el mismo orden que las variantes de `Lang`: el índice de la variante
/// es el índice de su fila.
pub const TODOS: &[Lang] = &[
    Lang::Rust,
    Lang::Python,
    Lang::Json,
    Lang::Toml,
    Lang::Markdown,
    Lang::Bash,
    Lang::C,
    Lang::Cpp,
    Lang::Css,
    Lang::Go,
    Lang::Html,
    Lang::Java,
    Lang::JavaScript,
    Lang::Lua,
    Lang::TypeScript,
    Lang::Tsx,
    Lang::Yaml,
];

/// La consulta de JavaScript incluye la de JSX: el mismo archivo `.js` puede
/// traer etiquetas, y la gramática las entiende.
fn consulta_js() -> String {
    format!(
        "{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
    )
}

static LENGUAJES: &[LangDef] = &[
    LangDef {
        id: "rust",
        label: "Rust",
        exts: &["rs"],
        filenames: &[],
        alias: &["rust"],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: Some("rust-analyzer"),
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "python",
        label: "Python",
        exts: &["py", "pyi"],
        filenames: &[],
        alias: &["python"],
        line_comment: Some("#"),
        indents_after_colon: true,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "json",
        label: "JSON",
        exts: &["json"],
        filenames: &[],
        alias: &[],
        line_comment: None,
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_json::LANGUAGE.into(),
                tree_sitter_json::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "toml",
        label: "TOML",
        exts: &["toml"],
        filenames: &[],
        alias: &[],
        line_comment: Some("#"),
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_toml_ng::LANGUAGE.into(),
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "markdown",
        label: "Markdown",
        exts: &["md", "markdown"],
        filenames: &[],
        alias: &[],
        line_comment: None,
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: tree_sitter_md::INJECTION_QUERY_BLOCK,
        grammar: || {
            (
                tree_sitter_md::LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_string(),
            )
        },
    },
    LangDef {
        id: "shellscript",
        label: "Shell",
        exts: &["sh", "bash", "zsh", "ksh"],
        filenames: &[".bashrc", ".bash_profile", ".bash_aliases", ".profile", ".zshrc"],
        alias: &["shell", "console"],
        line_comment: Some("#"),
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_bash::LANGUAGE.into(),
                tree_sitter_bash::HIGHLIGHT_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "c",
        label: "C",
        exts: &["c", "h"],
        filenames: &[],
        alias: &[],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: Some("clangd"),
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_c::LANGUAGE.into(),
                tree_sitter_c::HIGHLIGHT_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "cpp",
        label: "C++",
        exts: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
        filenames: &[],
        alias: &["c++"],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: Some("clangd"),
        // La consulta de C++ es un delta sobre la de C, igual que la de
        // TypeScript sobre la de JavaScript: sola no captura ni un `int`.
        // La de C va primero para que los patrones propios de C++ ganen.
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_cpp::LANGUAGE.into(),
                format!(
                    "{}\n{}",
                    tree_sitter_c::HIGHLIGHT_QUERY,
                    tree_sitter_cpp::HIGHLIGHT_QUERY
                ),
            )
        },
    },
    LangDef {
        id: "css",
        label: "CSS",
        exts: &["css"],
        filenames: &[],
        alias: &[],
        // CSS no tiene comentario de una línea: el único es `/* */`, que
        // abre y cierra, así que no entra en "poner un token adelante".
        line_comment: None,
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_css::LANGUAGE.into(),
                tree_sitter_css::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "go",
        label: "Go",
        exts: &["go"],
        filenames: &[],
        alias: &["golang"],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: Some("gopls"),
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_go::LANGUAGE.into(),
                tree_sitter_go::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "html",
        label: "HTML",
        exts: &["html", "htm", "xhtml"],
        filenames: &[],
        alias: &[],
        line_comment: None,
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: tree_sitter_html::INJECTIONS_QUERY,
        grammar: || {
            (
                tree_sitter_html::LANGUAGE.into(),
                tree_sitter_html::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "java",
        label: "Java",
        exts: &["java"],
        filenames: &[],
        alias: &[],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_java::LANGUAGE.into(),
                tree_sitter_java::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "javascript",
        label: "JavaScript",
        exts: &["js", "mjs", "cjs", "jsx"],
        filenames: &[],
        alias: &["node"],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: tree_sitter_javascript::INJECTIONS_QUERY,
        grammar: || (tree_sitter_javascript::LANGUAGE.into(), consulta_js()),
    },
    LangDef {
        id: "lua",
        label: "Lua",
        exts: &["lua"],
        filenames: &[],
        alias: &[],
        line_comment: Some("--"),
        indents_after_colon: false,
        lsp_command: Some("lua-language-server"),
        inyecciones: tree_sitter_lua::INJECTIONS_QUERY,
        grammar: || {
            (
                tree_sitter_lua::LANGUAGE.into(),
                tree_sitter_lua::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
    LangDef {
        id: "typescript",
        label: "TypeScript",
        exts: &["ts", "mts", "cts"],
        filenames: &[],
        alias: &[],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: None,
        // La consulta de TypeScript es un delta: va *después* de la de
        // JavaScript para que sus patrones (los tipos, sobre todo) ganen,
        // que es la regla de tree-sitter — manda el último que captura.
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                    tree_sitter_typescript::HIGHLIGHTS_QUERY
                ),
            )
        },
    },
    LangDef {
        id: "typescriptreact",
        label: "TSX",
        exts: &["tsx"],
        filenames: &[],
        alias: &[],
        line_comment: Some("//"),
        indents_after_colon: false,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                format!("{}\n{}", consulta_js(), tree_sitter_typescript::HIGHLIGHTS_QUERY),
            )
        },
    },
    LangDef {
        id: "yaml",
        label: "YAML",
        exts: &["yaml", "yml"],
        filenames: &[],
        alias: &[],
        line_comment: Some("#"),
        // Una clave suelta terminada en `:` abre un bloque, igual que en
        // Python.
        indents_after_colon: true,
        lsp_command: None,
        inyecciones: "",
        grammar: || {
            (
                tree_sitter_yaml::LANGUAGE.into(),
                tree_sitter_yaml::HIGHLIGHTS_QUERY.to_string(),
            )
        },
    },
];

impl Lang {
    fn def(&self) -> &'static LangDef {
        &LENGUAJES[*self as usize]
    }

    pub fn label(&self) -> &'static str {
        self.def().label
    }

    /// El identificador que usa LSP para este lenguaje, y también la clave
    /// con la que se lo nombra en `[lsp]` y `[files]` del config.toml.
    pub fn id(&self) -> &'static str {
        self.def().id
    }

    /// Cómo se escribe un comentario de una línea en este lenguaje. `None`
    /// donde el lenguaje no tiene: JSON no los admite (comentar una línea
    /// rompería el archivo), y en HTML, CSS y Markdown el único comentario
    /// abre y cierra, así que no entra en "poner un token adelante".
    pub fn line_comment(&self) -> Option<&'static str> {
        self.def().line_comment
    }

    /// Si en este lenguaje una línea terminada en `:` abre un bloque.
    pub fn indents_after_colon(&self) -> bool {
        self.def().indents_after_colon
    }

    /// Comando del servidor LSP para este lenguaje, si Flint sabe de uno que
    /// arranque sin argumentos.
    pub fn lsp_command(&self) -> Option<&'static str> {
        self.def().lsp_command
    }
}

/// El lenguaje que se pide por nombre: el de un cerco de código de Markdown
/// (```rust) o el de un shebang. Acepta el identificador, cualquiera de sus
/// extensiones y sus alias, que es como se escriben en la práctica.
pub fn lang_for_name(nombre: &str) -> Option<Lang> {
    let n = nombre.to_ascii_lowercase();
    let n = n.as_str();
    TODOS.iter().copied().find(|l| {
        let d = l.def();
        d.id == n || d.exts.contains(&n) || d.alias.contains(&n)
    })
}

/// El lenguaje que declara un shebang, para los archivos sin extensión —
/// un script llamado `deploy` a secas no tiene de dónde sacarlo si no es de
/// su primera línea.
///
/// Entiende tanto `#!/usr/bin/python3` como `#!/usr/bin/env python3 -u`, y
/// le saca la versión al nombre (`python3.11` → `python`) porque el número
/// cambia con la máquina y el lenguaje no.
pub fn lang_for_first_line(line: &str) -> Option<Lang> {
    let resto = line.trim_start().strip_prefix("#!")?;
    let mut palabras = resto.split_whitespace();
    let primero = palabras.next()?;
    let mut nombre = Path::new(primero).file_name()?.to_str()?;
    // `env` no es el intérprete: es quien lo busca en el PATH. El de verdad
    // es la primera palabra siguiente que no sea una opción de `env`.
    if nombre == "env" {
        nombre = palabras.find(|p| !p.starts_with('-'))?;
    }
    let base = nombre.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    lang_for_name(base)
}

/// El lenguaje de un archivo por su nombre. Primero el nombre completo, que
/// es lo que identifica a los que no tienen extensión (`.bashrc`), y después
/// la extensión.
pub fn lang_for_path(path: &Path) -> Option<Lang> {
    if let Some(nombre) = path.file_name().and_then(|n| n.to_str()) {
        let n = nombre.to_ascii_lowercase();
        if let Some(lang) = TODOS
            .iter()
            .copied()
            .find(|l| l.def().filenames.contains(&n.as_str()))
        {
            return Some(lang);
        }
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    TODOS
        .iter()
        .copied()
        .find(|l| l.def().exts.contains(&ext.as_str()))
}

const CAPTURE_NAMES: &[&str] = &[
    "keyword",
    "function",
    "function.method",
    "function.macro",
    "type",
    "type.builtin",
    "string",
    "string.special",
    "comment",
    "number",
    "constant",
    "constant.builtin",
    "boolean",
    "operator",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "variable",
    "variable.parameter",
    "variable.builtin",
    "property",
    "attribute",
    // Capturas de texto con formato, que usan las gramáticas de Markdown.
    // Una captura que no esté en esta lista simplemente no se colorea: es lo
    // que pasaba con todo el texto de un .md salvo sus signos de puntuación.
    "text.title",
    "text.strong",
    "text.emphasis",
    "text.literal",
    "text.uri",
    "text.reference",
    "string.escape",
    // Las de acá abajo salieron de auditar las diecisiete gramáticas: son
    // nombres que sus consultas usan de verdad y que Flint venía tirando.
    // Y tirar una captura no es solo "no pintar esa": manda la última que
    // toca el nodo, así que una captura desconocida al final le sacaba el
    // color que le había puesto una anterior. Lua era el caso extremo, sus
    // `if`/`for`/`while` salen como `conditional` y `repeat`, así que se
    // veía casi sin colores.
    "escape",
    "constructor",
    "label",
    "delimiter",
    "tag",
    "embedded",
    "conditional",
    "repeat",
    "field",
    "method",
    "parameter",
    "preproc",
    // Todavía no las usa ninguna de las diecisiete, pero son nombres
    // estándar de tree-sitter: tenerlas evita que la próxima gramática que
    // se agregue llegue a medio pintar.
    "namespace",
    "include",
    "exception",
    "character",
    "float",
    "annotation",
];

/// Capturas que se ignoran a propósito, no por olvido. `@none` es el nombre
/// con el que una consulta dice "esto no se pinta": el contenido de un cerco
/// de código de Markdown, por ejemplo, que se colorea aparte con la
/// gramática del lenguaje que declara.
#[cfg(test)]
const CAPTURAS_IGNORADAS: &[&str] = &["none"];

fn kind_for_capture(name: &str) -> HighlightKind {
    if name.starts_with("keyword") {
        HighlightKind::Keyword
    } else if name.starts_with("function") {
        HighlightKind::Function
    } else if name.starts_with("type") {
        HighlightKind::Type
    } else if name.starts_with("string") {
        HighlightKind::String
    } else if name == "comment" {
        HighlightKind::Comment
    } else if name == "number" {
        HighlightKind::Number
    } else if name.starts_with("constant") || name == "boolean" {
        HighlightKind::Constant
    } else if name == "operator" {
        HighlightKind::Operator
    } else if name.starts_with("punctuation") || name == "delimiter" {
        HighlightKind::Punctuation
    } else if name == "constructor" || name == "namespace" {
        // Un constructor nombra un tipo (`Some`, `Ok`, una struct literal),
        // así que va del mismo color que el tipo.
        HighlightKind::Type
    } else if name == "escape" || name == "character" {
        HighlightKind::String
    } else if name == "float" {
        HighlightKind::Number
    } else if name == "label" {
        HighlightKind::Constant
    } else if name == "tag" {
        // El nombre de una etiqueta de HTML, del mismo color que una palabra
        // clave: es lo que hace casi cualquier editor y lo que uno espera al
        // mirar un `.html`.
        HighlightKind::Keyword
    } else if name == "conditional"
        || name == "repeat"
        || name == "include"
        || name == "exception"
        || name == "preproc"
    {
        HighlightKind::Keyword
    } else if name == "method" {
        HighlightKind::Function
    } else if name == "field" {
        HighlightKind::Property
    } else if name == "annotation" {
        HighlightKind::Attribute
    } else if name == "text.title" {
        HighlightKind::Heading
    } else if name == "text.strong" {
        HighlightKind::Strong
    } else if name == "text.emphasis" {
        HighlightKind::Emphasis
    } else if name == "text.uri" || name == "text.reference" {
        HighlightKind::Link
    } else if name == "text.literal" {
        // El código —en línea o en un cerco— con el mismo color que un
        // literal de texto en cualquier otro lenguaje.
        HighlightKind::String
    } else if name == "property" {
        HighlightKind::Property
    } else if name == "attribute" {
        HighlightKind::Attribute
    } else {
        HighlightKind::Variable
    }
}

/// El nombre de `CAPTURE_NAMES` que le corresponde a una captura de la
/// consulta, o `None` si ninguno — en cuyo caso esa captura no se colorea.
///
/// La regla es la misma que usaba `HighlightConfiguration::configure`: los
/// segmentos del nombre reconocido tienen que ser un prefijo de los de la
/// captura (`string` reconoce `string.special.path`), y entre varios que
/// calcen gana el más largo (`function.method` le gana a `function`).
fn reconocida(captura: &str) -> Option<&'static str> {
    CAPTURE_NAMES
        .iter()
        .filter(|n| {
            captura == **n
                || (captura.len() > n.len()
                    && captura.starts_with(**n)
                    && captura.as_bytes()[n.len()] == b'.')
        })
        .max_by_key(|n| n.len())
        .copied()
}

// ---------- leer el rope sin copiarlo ----------

/// Los trozos contiguos de un tramo del rope, como bytes. Es lo que pide
/// tree-sitter para leer el texto de un nodo cuando una consulta tiene
/// predicados (`#eq?`, `#match?`, `#any-of?`).
pub struct TrozosBytes<'a>(ropey::iter::Chunks<'a>);

impl<'a> Iterator for TrozosBytes<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        self.0.next().map(str::as_bytes)
    }
}

/// El rope como fuente de texto de una consulta, sin materializar el
/// documento: se le entregan los trozos que el rope ya tiene en memoria.
struct FuenteRope<'a>(&'a Rope);

impl<'a> TextProvider<&'a [u8]> for FuenteRope<'a> {
    type I = TrozosBytes<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        let fin = node.end_byte().min(self.0.len_bytes());
        let ini = node.start_byte().min(fin);
        TrozosBytes(self.0.byte_slice(ini..fin).chunks())
    }
}

/// Analiza el rope leyéndolo por trozos. `viejo` es el árbol de la pasada
/// anterior, ya editado con `Tree::edit`: con él tree-sitter reusa todo el
/// subárbol que la edición no tocó en vez de rehacerlo.
fn parsear(parser: &mut Parser, rope: &Rope, viejo: Option<&Tree>) -> Option<Tree> {
    let mut leer = |byte: usize, _: Point| -> &[u8] {
        if byte >= rope.len_bytes() {
            return &[];
        }
        let (trozo, inicio, _, _) = rope.chunk_at_byte(byte);
        &trozo.as_bytes()[byte - inicio..]
    };
    parser.parse_with_options(&mut leer, viejo, None)
}

/// La posición de un byte en fila y columna, que es como tree-sitter mide.
/// Ojo: la columna va en bytes dentro de la línea, no en caracteres.
fn punto(rope: &Rope, byte: usize) -> Point {
    let byte = byte.min(rope.len_bytes());
    let fila = rope.byte_to_line(byte);
    Point {
        row: fila,
        column: byte - rope.line_to_byte(fila),
    }
}

/// Los caracteres de una línea sin contar su salto (ni `\n` ni `\r\n`) —
/// la misma cuenta que `Editor::line_char_len`, hecha sobre el rope suelto.
fn largo_linea(rope: &Rope, linea: usize) -> usize {
    let slice = rope.line(linea);
    let len = slice.len_chars();
    if len == 0 {
        return 0;
    }
    if slice.char(len - 1) == '\n' {
        if len >= 2 && slice.char(len - 2) == '\r' {
            len - 2
        } else {
            len - 1
        }
    } else {
        len
    }
}

// ---------- una gramática y su consulta de resaltado ----------

/// Una gramática compilada junto con su consulta de resaltado y la
/// traducción de cada captura a la categoría que entiende la UI.
struct Gramatica {
    language: Language,
    query: Query,
    /// Por índice de captura de la consulta: su categoría, o `None` si esa
    /// captura no está en `CAPTURE_NAMES` y entonces no se pinta.
    kinds: Vec<Option<HighlightKind>>,
}

impl Gramatica {
    fn nueva(language: Language, consulta: &str) -> Option<Gramatica> {
        let query = Query::new(&language, consulta).ok()?;
        let kinds = query
            .capture_names()
            .iter()
            .map(|n| reconocida(n).map(kind_for_capture))
            .collect();
        Some(Gramatica {
            language,
            query,
            kinds,
        })
    }

    /// Las capturas de la consulta que tocan `span`, ya aplanadas: rangos sin
    /// solapamiento, cada byte con la categoría de la captura más interna.
    ///
    /// `span` no es solo un filtro de salida: se lo pasa al cursor de la
    /// consulta, así que en una edición chica tree-sitter recorre un tramo
    /// del árbol y no el árbol entero. Ese es el otro lado del incremental —
    /// reusar el árbol no sirve de nada si después hay que volver a
    /// consultarlo completo.
    fn resaltar<T, I>(
        &self,
        arbol: &Tree,
        texto: T,
        span: Range<usize>,
    ) -> Vec<(Range<usize>, HighlightKind)>
    where
        T: TextProvider<I>,
        I: AsRef<[u8]>,
    {
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(span.clone());
        let mut capturas: Vec<(usize, usize, HighlightKind)> = Vec::new();
        // Un mismo nodo puede quedar capturado por varios patrones de la
        // consulta, y manda el último: es la regla de tree-sitter (los
        // patrones de más abajo pisan a los de más arriba). Si esa última
        // captura no está en `CAPTURE_NAMES`, el nodo no se colorea —
        // aunque una captura anterior sí estuviera.
        let mut ultima: Option<(usize, usize, usize, Option<HighlightKind>)> = None;
        let mut it = cursor.captures(&self.query, arbol.root_node(), texto);
        while let Some(c) = it.next() {
            let captura = c.0.captures()[c.1];
            let kind = self.kinds.get(captura.index as usize).copied().flatten();
            let id = captura.node.id();
            if let Some(u) = &mut ultima
                && u.0 == id
            {
                u.3 = kind;
                continue;
            }
            if let Some((_, desde, hasta, Some(k))) = ultima {
                capturas.push((desde, hasta, k));
            }
            ultima = Some((id, captura.node.start_byte(), captura.node.end_byte(), kind));
        }
        if let Some((_, desde, hasta, Some(k))) = ultima {
            capturas.push((desde, hasta, k));
        }
        aplanar(capturas, span)
    }
}

/// Arma la gramática de un lenguaje inyectado a partir del nombre con el que
/// la consulta lo pidió.
///
/// `markdown_inline` no es un lenguaje de la tabla —no se abre un archivo
/// `.markdown_inline`— pero es el nombre con el que la consulta de Markdown
/// pide su propia segunda pasada, así que se resuelve aparte.
fn armar_inyectada(nombre: &str) -> Option<Gramatica> {
    if nombre == "markdown_inline" {
        return Gramatica::nueva(
            tree_sitter_md::INLINE_LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
        );
    }
    let lang = lang_for_name(nombre)?;
    let (language, query) = (lang.def().grammar)();
    Gramatica::nueva(language, &query)
}

/// Convierte las capturas en tramos que no se pisan: cada byte se queda con
/// la categoría de la captura que esté más arriba en la pila.
///
/// Las capturas de un mismo árbol son nodos, así que o se contienen o son
/// disjuntas; alcanza con recorrerlas de izquierda a derecha manteniendo una
/// pila de las que siguen abiertas. Las capturas vacías (las gramáticas de
/// Markdown tienen varias) no pintan nada pero sí cortan un tramo en dos,
/// y se conservan por eso.
fn aplanar(
    mut capturas: Vec<(usize, usize, HighlightKind)>,
    span: Range<usize>,
) -> Vec<(Range<usize>, HighlightKind)> {
    capturas.retain(|&(s, e, _)| e >= span.start && s <= span.end);
    // Solo por dónde empieza, y estable: entre dos capturas que arrancan en
    // el mismo byte manda el orden en que las emitió la consulta, que es el
    // de los patrones. No se ordena por largo a propósito — hacerlo cambia
    // qué gana cuando un nodo y su padre empiezan juntos, y con eso cambian
    // colores que llevan versiones siendo los mismos.
    capturas.sort_by_key(|c| c.0);

    let mut salida: Vec<(Range<usize>, HighlightKind)> = Vec::new();
    let mut pila: Vec<(usize, HighlightKind)> = Vec::new();
    let mut pos = span.start;
    let mut i = 0usize;

    loop {
        let inicio = capturas.get(i).map(|c| c.0);
        let cierre = pila.last().map(|&(f, _)| f);
        // El próximo borde: o empieza una captura, o termina la que está
        // arriba de la pila — lo que venga primero.
        let (corte, abre) = match (inicio, cierre) {
            (Some(s), Some(f)) if f <= s => (f, false),
            (Some(s), _) => (s, true),
            (None, Some(f)) => (f, false),
            (None, None) => break,
        };
        if let Some(&(_, kind)) = pila.last() {
            let desde = pos.max(span.start);
            let hasta = corte.min(span.end);
            if desde < hasta {
                salida.push((desde..hasta, kind));
            }
        }
        pos = pos.max(corte);
        if abre {
            let (_, fin, kind) = capturas[i];
            i += 1;
            pila.push((fin, kind));
        } else {
            pila.pop();
        }
    }
    salida
}

// ---------- el resaltador de un buffer ----------

/// El árbol de la última pasada y el texto exacto del que salió. El texto se
/// guarda como un clon del rope, que en ropey comparte las hojas con el
/// original y por lo tanto no duplica el documento en memoria.
struct Cache {
    texto: Rope,
    arbol: Tree,
}

pub struct LanguageHighlighter {
    principal: Gramatica,
    /// La consulta que dice qué tramos delega este lenguaje en otro. Es lo
    /// que hace que el `<style>` de un HTML salga como CSS, el `<script>`
    /// como JavaScript, y el cerco de código de un Markdown como el lenguaje
    /// que declara. Markdown se apoya además en esto para su propia segunda
    /// pasada: el árbol de bloques marca los tramos `inline` (lo de adentro
    /// de una línea, `**negrita**`, `` `código` ``, links) como una
    /// inyección de `markdown_inline`, que es una gramática aparte.
    inyecciones: Option<Query>,
    /// Las gramáticas de los lenguajes inyectados, armadas la primera vez
    /// que aparecen y guardadas: compilar una consulta no es gratis y un
    /// documento puede tener veinte cercos del mismo lenguaje. El `None`
    /// guardado también cuenta — es "este nombre no lo conozco", y evita
    /// volver a intentarlo en cada tecla.
    inyectadas: RefCell<HashMap<String, Option<Rc<Gramatica>>>>,
    parser: Parser,
    cache: Option<Cache>,
}

/// Hasta dónde se sigue una inyección adentro de otra. Un Markdown puede
/// traer un cerco de HTML que a su vez trae un `<script>`; más hondo que eso
/// no aporta nada y una gramática que se inyecte a sí misma colgaría el
/// editor.
const MAX_PROFUNDIDAD: usize = 3;

impl LanguageHighlighter {
    pub fn new(lang: &Lang) -> Option<Self> {
        let (language, query) = (lang.def().grammar)();
        let principal = Gramatica::nueva(language, &query)?;
        let fuente_iny = lang.def().inyecciones;
        let inyecciones = if fuente_iny.is_empty() {
            None
        } else {
            // Que la consulta de inyecciones no compile no tiene por qué
            // dejar el archivo sin color: se pierde el `<style>` como CSS,
            // no el HTML entero.
            Query::new(&principal.language, fuente_iny).ok()
        };
        let mut parser = Parser::new();
        parser.set_language(&principal.language).ok()?;
        Some(LanguageHighlighter {
            principal,
            inyecciones,
            inyectadas: RefCell::new(HashMap::new()),
            parser,
            cache: None,
        })
    }

    /// La gramática de un lenguaje inyectado, por el nombre con el que la
    /// consulta lo pidió (```rust, `injection.language "css"`). `None` si no
    /// es un lenguaje que Flint conozca, que es lo normal: las consultas
    /// nombran muchos más de los que hay.
    fn gramatica_inyectada(&self, nombre: &str) -> Option<Rc<Gramatica>> {
        if let Some(g) = self.inyectadas.borrow().get(nombre) {
            return g.clone();
        }
        let armada = armar_inyectada(nombre).map(Rc::new);
        self.inyectadas
            .borrow_mut()
            .insert(nombre.to_string(), armada.clone());
        armada
    }

    /// El árbol vigente para este buffer, si el que quedó en caché salió
    /// exactamente de `rope`. Sirve para que la selección estructural use el
    /// árbol que el resaltado ya calculó en vez de analizar el archivo otra
    /// vez.
    pub fn arbol_de(&self, rope: &Rope) -> Option<&Tree> {
        let cache = self.cache.as_ref()?;
        (cache.texto == *rope).then_some(&cache.arbol)
    }

    /// Analiza un texto suelto y devuelve su árbol. Es el camino para lo que
    /// no es el buffer: un fragmento, o el buffer cuando todavía no hay nada
    /// en caché.
    pub fn parse(&self, source: &str) -> Option<Tree> {
        let mut parser = Parser::new();
        parser.set_language(&self.principal.language).ok()?;
        parser.parse(source, None)
    }

    /// El resaltado de un texto suelto, agrupado por línea y en columnas de
    /// carácter — la misma forma que `Editor::highlights_by_line`, pero para
    /// un fragmento que no es un buffer (el contenido de un cerco de código
    /// dentro de la vista previa de Markdown).
    pub fn highlight_lines(&self, source: &str) -> Vec<Vec<(usize, usize, HighlightKind)>> {
        let rope = Rope::from_str(source);
        let lineas = rope.len_lines().max(1);
        let mut salida = vec![Vec::new(); lineas];
        let spans = match self.parse(source) {
            Some(arbol) => self.resaltar(&arbol, &rope, 0..rope.len_bytes(), 0),
            None => Vec::new(),
        };
        volcar(&mut salida, &rope, spans, 0..=lineas - 1);
        salida
    }

    /// El resaltado de un texto suelto como tramos de bytes, sin repartir por
    /// línea. Analiza el texto de cero: es el camino de los fragmentos, no el
    /// del buffer.
    /// Solo la gramática principal, sin las inyecciones. Es lo que se compara
    /// contra el motor viejo: `tree-sitter-highlight` tampoco las seguía (se
    /// le pasaba un callback que devolvía `None` siempre), así que es el
    /// recorrido de la consulta lo que las dos versiones tienen en común.
    #[cfg(test)]
    fn spans_base(&self, source: &str) -> Vec<(Range<usize>, HighlightKind)> {
        let Some(arbol) = self.parse(source) else {
            return Vec::new();
        };
        let rope = Rope::from_str(source);
        self.principal
            .resaltar(&arbol, FuenteRope(&rope), 0..rope.len_bytes())
    }

    #[cfg(test)]
    fn spans_texto(&self, source: &str) -> Vec<(Range<usize>, HighlightKind)> {
        let Some(arbol) = self.parse(source) else {
            return Vec::new();
        };
        let rope = Rope::from_str(source);
        self.resaltar(&arbol, &rope, 0..rope.len_bytes(), 0)
    }

    /// El resaltado de un tramo: la gramática principal, y encima los tramos
    /// que este lenguaje delega en otra gramática.
    ///
    /// El orden importa y es el de siempre: lo inyectado va después, y al
    /// pintar gana lo último. Así el `<script>` de un HTML queda con los
    /// colores de JavaScript encima de los que le puso el HTML, y el
    /// `**negrita**` de un Markdown por encima del párrafo.
    fn resaltar(
        &self,
        arbol: &Tree,
        rope: &Rope,
        span: Range<usize>,
        profundidad: usize,
    ) -> Vec<(Range<usize>, HighlightKind)> {
        let mut spans = self.principal.resaltar(arbol, FuenteRope(rope), span.clone());
        if profundidad + 1 >= MAX_PROFUNDIDAD {
            return spans;
        }
        for (rango, nombre) in self.rangos_inyectados(arbol, rope, &span) {
            let Some(gramatica) = self.gramatica_inyectada(&nombre) else {
                continue;
            };
            let fragmento: Cow<str> = rope.byte_slice(rango.clone()).into();
            let sub = Rope::from_str(&fragmento);
            let mut parser = Parser::new();
            if parser.set_language(&gramatica.language).is_err() {
                continue;
            }
            let Some(sub_arbol) = parsear(&mut parser, &sub, None) else {
                continue;
            };
            let offset = rango.start;
            let interno = gramatica.resaltar(&sub_arbol, FuenteRope(&sub), 0..sub.len_bytes());
            spans.extend(
                interno
                    .into_iter()
                    .map(|(r, k)| (r.start + offset..r.end + offset, k)),
            );
        }
        spans
    }

    /// Los tramos que la consulta de inyecciones marca dentro de `span`, con
    /// el nombre del lenguaje que les toca. El nombre puede venir de una
    /// captura (```rust, donde la palabra está en el propio documento) o de
    /// un `#set!` de la consulta (el `<style>` de un HTML, que siempre es
    /// CSS).
    fn rangos_inyectados(
        &self,
        arbol: &Tree,
        rope: &Rope,
        span: &Range<usize>,
    ) -> Vec<(Range<usize>, String)> {
        let Some(query) = &self.inyecciones else {
            return Vec::new();
        };
        let nombres = query.capture_names();
        let mut salida = Vec::new();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(span.clone());
        let mut it = cursor.matches(query, arbol.root_node(), FuenteRope(rope));
        while let Some(m) = it.next() {
            let mut contenido: Option<Range<usize>> = None;
            let mut lenguaje: Option<String> = None;
            for captura in m.captures() {
                match nombres.get(captura.index as usize).copied() {
                    Some("injection.content") => {
                        contenido = Some(captura.node.byte_range());
                    }
                    Some("injection.language") => {
                        let r = captura.node.byte_range();
                        let texto: Cow<str> = rope.byte_slice(r).into();
                        lenguaje = Some(texto.trim().to_string());
                    }
                    _ => {}
                }
            }
            if lenguaje.is_none() {
                lenguaje = query
                    .property_settings(m.pattern_index)
                    .iter()
                    .find(|p| &*p.key == "injection.language")
                    .and_then(|p| p.value.as_deref().map(str::to_string));
            }
            if let (Some(rango), Some(nombre)) = (contenido, lenguaje)
                && rango.end > rango.start
                && rango.end <= rope.len_bytes()
                && !nombre.is_empty()
            {
                salida.push((rango, nombre));
            }
        }
        salida
    }

    /// Rehace el resaltado entero. Es el camino de la primera pasada y el de
    /// vuelta cuando algo no cuadra: siempre correcto, nunca barato.
    fn completo(&mut self, ed: &mut Editor, rope: Rope) {
        let lineas = rope.len_lines().max(1);
        let mut by_line = vec![Vec::new(); lineas];
        self.cache = None;
        if let Some(arbol) = parsear(&mut self.parser, &rope, None) {
            let spans = self.resaltar(&arbol, &rope, 0..rope.len_bytes(), 0);
            volcar(&mut by_line, &rope, spans, 0..=lineas - 1);
            self.cache = Some(Cache { texto: rope, arbol });
        }
        ed.highlights_by_line = by_line;
    }

    /// Deja `ed.highlights_by_line` al día reusando todo lo que se pueda del
    /// árbol y del resaltado de la pasada anterior.
    fn actualizar(&mut self, ed: &mut Editor) {
        let rope = ed.rope.clone();
        let Some(cache) = self.cache.take() else {
            self.completo(ed, rope);
            return;
        };
        // Si lo dibujado no corresponde al texto del que salió el árbol, no
        // hay de dónde partir.
        if ed.highlights_by_line.len() != cache.texto.len_lines().max(1) {
            self.completo(ed, rope);
            return;
        }
        let Some((inicio, fin_viejo, fin_nuevo)) = diferencia(&cache.texto, &rope) else {
            // Mismo texto: el árbol y el resaltado que ya están siguen valiendo.
            self.cache = Some(cache);
            return;
        };

        let edicion = InputEdit {
            start_byte: inicio,
            old_end_byte: fin_viejo,
            new_end_byte: fin_nuevo,
            start_position: punto(&cache.texto, inicio),
            old_end_position: punto(&cache.texto, fin_viejo),
            new_end_position: punto(&rope, fin_nuevo),
        };
        let mut arbol_viejo = cache.arbol;
        arbol_viejo.edit(&edicion);
        let Some(arbol) = parsear(&mut self.parser, &rope, Some(&arbol_viejo)) else {
            self.completo(ed, rope);
            return;
        };

        // Qué hay que volver a consultar: el tramo editado, más todo lo que
        // tree-sitter dice que cambió de forma. Lo segundo es lo que cubre
        // los cambios a distancia — escribir `/*` recolorea hasta donde ese
        // comentario termine, y el tramo editado son dos bytes.
        let mut sucio = inicio..fin_nuevo.max(inicio);
        for r in arbol_viejo.changed_ranges(&arbol) {
            sucio.start = sucio.start.min(r.start_byte);
            sucio.end = sucio.end.max(r.end_byte);
        }
        sucio.end = sucio.end.min(rope.len_bytes());
        sucio.start = sucio.start.min(sucio.end);

        // Alinear las líneas guardadas con el documento nuevo: las de antes
        // del corte no se movieron, las de después solo cambiaron de número.
        let l_ini = cache.texto.byte_to_line(inicio.min(cache.texto.len_bytes()));
        let l_fin_viejo = cache
            .texto
            .byte_to_line(fin_viejo.min(cache.texto.len_bytes()));
        let l_fin_nuevo = rope.byte_to_line(fin_nuevo.min(rope.len_bytes()));
        let nuevas = (l_fin_nuevo + 1).saturating_sub(l_ini);
        ed.highlights_by_line
            .splice(l_ini..=l_fin_viejo, (0..nuevas).map(|_| Vec::new()));

        let lineas = rope.len_lines().max(1);
        if ed.highlights_by_line.len() != lineas {
            self.completo(ed, rope);
            return;
        }

        // La ventana a recalcular, en líneas enteras: una columna a medias no
        // sirve porque el resaltado se guarda por línea.
        let v0 = rope.byte_to_line(sucio.start).min(l_ini);
        let v1 = rope
            .byte_to_line(sucio.end)
            .max(l_fin_nuevo)
            .min(lineas - 1);
        for linea in v0..=v1 {
            ed.highlights_by_line[linea].clear();
        }
        let b0 = rope.line_to_byte(v0);
        let b1 = if v1 + 1 < lineas {
            rope.line_to_byte(v1 + 1)
        } else {
            rope.len_bytes()
        };
        let spans = self.resaltar(&arbol, &rope, b0..b1, 0);
        volcar(&mut ed.highlights_by_line, &rope, spans, v0..=v1);
        self.cache = Some(Cache { texto: rope, arbol });
    }
}

/// Reparte tramos de bytes en el resaltado por línea, en columnas de
/// carácter, sin tocar las líneas de afuera de `ventana`.
fn volcar(
    by_line: &mut [Vec<(usize, usize, HighlightKind)>],
    rope: &Rope,
    spans: Vec<(Range<usize>, HighlightKind)>,
    ventana: std::ops::RangeInclusive<usize>,
) {
    let len_bytes = rope.len_bytes();
    let len_chars = rope.len_chars();
    let tope = by_line.len().saturating_sub(1);
    for (rango, kind) in spans {
        let inicio_char = rope.byte_to_char(rango.start.min(len_bytes));
        let fin_char = rope.byte_to_char(rango.end.min(len_bytes));
        if fin_char <= inicio_char {
            continue;
        }
        let primera = rope.char_to_line(inicio_char.min(len_chars));
        let ultima = rope.char_to_line((fin_char - 1).min(len_chars.saturating_sub(1)));
        let desde = primera.max(*ventana.start());
        let hasta = ultima.min(*ventana.end()).min(tope);
        if desde > hasta {
            continue;
        }
        for (linea, entradas) in by_line.iter_mut().enumerate().take(hasta + 1).skip(desde) {
            let li = rope.line_to_char(linea);
            let lf = li + largo_linea(rope, linea);
            let s = inicio_char.max(li);
            let e = fin_char.min(lf);
            if s < e {
                entradas.push((s - li, e - li, kind));
            }
        }
    }
}

// ---------- de qué cambió el texto ----------

/// El único tramo que cambió entre dos versiones del documento, en bytes:
/// `(desde, hasta en el viejo, hasta en el nuevo)`. `None` si son iguales.
///
/// Sale de comparar prefijo y sufijo comunes, y no de anotar cada mutación
/// del buffer en el lugar donde ocurre. Es una decisión, no una comodidad:
/// un `InputEdit` mal anotado —y hay como diez lugares que mutan el rope,
/// más deshacer, rehacer y multi-cursor— desincroniza el árbol en silencio,
/// y el resaltado se corrompe sin que nada avise. Derivarlo del texto no
/// puede mentir: a lo sumo describe un tramo más grande del que cambió de
/// verdad, y eso solo cuesta un poco más de reparseo.
///
/// Dos ediciones lejanas en una sola pasada (multi-cursor) entran acá como
/// un tramo que las abarca a las dos. Es correcto, y en la práctica es el
/// caso raro.
fn diferencia(viejo: &Rope, nuevo: &Rope) -> Option<(usize, usize, usize)> {
    let lv = viejo.len_bytes();
    let ln = nuevo.len_bytes();
    let mut ini = prefijo_comun(viejo, nuevo);
    if ini == lv && ini == ln {
        return None;
    }
    // Los cortes tienen que caer en frontera de carácter: dos textos pueden
    // compartir el primer byte de una `é` y diferir en el segundo.
    while ini > 0 && !frontera(nuevo, ini) {
        ini -= 1;
    }
    let tope = (lv - ini).min(ln - ini);
    let mut suf = sufijo_comun(viejo, nuevo, tope);
    while suf > 0 && !frontera(nuevo, ln - suf) {
        suf -= 1;
    }
    Some((ini, lv - suf, ln - suf))
}

fn frontera(rope: &Rope, byte: usize) -> bool {
    byte == 0 || byte == rope.len_bytes() || rope.char_to_byte(rope.byte_to_char(byte)) == byte
}

/// Cuántos bytes comparten dos ropes desde el principio.
///
/// El camino rápido es la clave: un clon de ropey comparte las hojas con el
/// original, así que los trozos que la edición no tocó son el *mismo* trozo
/// en memoria y se saltan sin compararlos.
fn prefijo_comun(a: &Rope, b: &Rope) -> usize {
    let mut ia = a.chunks();
    let mut ib = b.chunks();
    let (mut ca, mut cb): (&[u8], &[u8]) = (&[], &[]);
    let mut n = 0usize;
    loop {
        if ca.is_empty() {
            match ia.next() {
                Some(s) => {
                    ca = s.as_bytes();
                    continue;
                }
                None => break,
            }
        }
        if cb.is_empty() {
            match ib.next() {
                Some(s) => {
                    cb = s.as_bytes();
                    continue;
                }
                None => break,
            }
        }
        if ca.len() == cb.len() && std::ptr::eq(ca.as_ptr(), cb.as_ptr()) {
            n += ca.len();
            ca = &[];
            cb = &[];
            continue;
        }
        let m = ca.len().min(cb.len());
        let iguales = ca[..m]
            .iter()
            .zip(&cb[..m])
            .take_while(|(x, y)| x == y)
            .count();
        n += iguales;
        if iguales < m {
            break;
        }
        ca = &ca[m..];
        cb = &cb[m..];
    }
    n
}

/// Cuántos bytes comparten dos ropes desde el final, sin pasarse de `tope`
/// (lo que queda después del prefijo común, para que los dos tramos no se
/// superpongan).
fn sufijo_comun(a: &Rope, b: &Rope, tope: usize) -> usize {
    let (mut ia, _, _, _) = a.chunks_at_byte(a.len_bytes());
    let (mut ib, _, _, _) = b.chunks_at_byte(b.len_bytes());
    let (mut ca, mut cb): (&[u8], &[u8]) = (&[], &[]);
    let mut n = 0usize;
    while n < tope {
        if ca.is_empty() {
            match ia.prev() {
                Some(s) => {
                    ca = s.as_bytes();
                    continue;
                }
                None => break,
            }
        }
        if cb.is_empty() {
            match ib.prev() {
                Some(s) => {
                    cb = s.as_bytes();
                    continue;
                }
                None => break,
            }
        }
        if ca.len() == cb.len()
            && std::ptr::eq(ca.as_ptr(), cb.as_ptr())
            && n + ca.len() <= tope
        {
            n += ca.len();
            ca = &[];
            cb = &[];
            continue;
        }
        let m = ca.len().min(cb.len()).min(tope - n);
        let iguales = ca[ca.len() - m..]
            .iter()
            .rev()
            .zip(cb[cb.len() - m..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        n += iguales;
        if iguales < m {
            break;
        }
        ca = &ca[..ca.len() - m];
        cb = &cb[..cb.len() - m];
    }
    n.min(tope)
}

/// Recalcula `ed.highlights_by_line` a partir del contenido actual del buffer.
/// Solo hace trabajo real si `ed.highlights_dirty` está encendido.
pub fn refresh(ed: &mut Editor, highlighter: Option<&mut LanguageHighlighter>) {
    if !ed.highlights_dirty {
        return;
    }
    match highlighter {
        Some(h) => h.actualizar(ed),
        None => ed.highlights_by_line = vec![Vec::new(); ed.line_count().max(1)],
    }
    ed.highlights_dirty = false;
}

/// Direccionamiento estructural: dado un rango de bytes seleccionado, encuentra
/// el nodo del árbol de sintaxis más pequeño que lo contiene. Si la selección
/// ya coincide exactamente con un nodo, sube a su nodo padre — así presionar la
/// tecla repetidas veces va ampliando la selección un nivel del árbol por vez.
pub fn expand_selection(tree: &Tree, start_byte: usize, end_byte: usize) -> Option<(usize, usize)> {
    let root = tree.root_node();
    let node = root.descendant_for_byte_range(start_byte, end_byte)?;
    let target = if node.start_byte() == start_byte && node.end_byte() == end_byte {
        node.parent().unwrap_or(node)
    } else {
        node
    };
    Some((target.start_byte(), target.end_byte()))
}

#[cfg(test)]
mod tests_lenguajes {
    //! La tabla de lenguajes es una lista paralela al enum `Lang`, y las
    //! consultas de resaltado vienen de crates de terceros que pueden
    //! nombrar nodos que su gramática no tiene. Las dos cosas fallan en
    //! silencio si nadie las mira: un índice corrido daría el resaltado de
    //! otro lenguaje, y una consulta que no compila deja el archivo en
    //! blanco y negro sin decir nada.
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn la_tabla_esta_alineada_con_el_enum() {
        assert_eq!(TODOS.len(), LENGUAJES.len(), "faltó una fila o una variante");
        for (i, lang) in TODOS.iter().enumerate() {
            assert_eq!(*lang as usize, i, "{lang:?} no está en su posición");
        }
    }

    #[test]
    fn cada_lenguaje_compila_su_gramatica_y_su_consulta() {
        for lang in TODOS {
            let h = LanguageHighlighter::new(lang);
            assert!(
                h.is_some(),
                "{} no armó su resaltador: la consulta no compila contra su gramática",
                lang.label()
            );
        }
    }

    #[test]
    fn cada_lenguaje_resalta_algo_de_verdad() {
        // Compilar la consulta no alcanza: una consulta válida que no
        // capture nada del código real dejaría el archivo sin color igual.
        let muestras = MUESTRAS_CORTAS;
        assert_eq!(muestras.len(), TODOS.len(), "falta una muestra para un lenguaje nuevo");
        for (lang, fuente) in muestras {
            let h = LanguageHighlighter::new(lang).expect("la gramática compila");
            let spans = h.spans_texto(fuente);
            assert!(
                !spans.is_empty(),
                "{} no resaltó nada en su muestra",
                lang.label()
            );
        }
    }

    #[test]
    fn los_identificadores_y_las_extensiones_no_se_repiten() {
        let mut ids: HashSet<&str> = HashSet::new();
        let mut exts: HashSet<&str> = HashSet::new();
        let mut nombres: HashSet<String> = HashSet::new();
        for lang in TODOS {
            let d = lang.def();
            assert!(ids.insert(d.id), "el identificador {} está dos veces", d.id);
            for ext in d.exts {
                assert!(
                    exts.insert(ext),
                    "la extensión .{ext} está en dos lenguajes ({})",
                    lang.label()
                );
            }
            for nombre in d.filenames {
                assert_eq!(
                    *nombre,
                    nombre.to_ascii_lowercase(),
                    "{nombre} tiene que estar en minúscula: se compara así"
                );
                assert!(
                    nombres.insert(nombre.to_string()),
                    "el nombre de archivo {nombre} está en dos lenguajes"
                );
            }
        }
    }

    #[test]
    fn ninguna_gramatica_pierde_capturas_en_silencio() {
        // Este test existe por lo que pasó al sumar las doce gramáticas
        // nuevas: varias usan nombres de captura que Flint no reconocía, y
        // el efecto no era "esa captura no pinta" sino "el nodo se queda sin
        // color", porque manda la última captura que lo toca. Lua se veía
        // casi en blanco y negro y nada lo avisaba.
        let mut faltantes: Vec<String> = Vec::new();
        for lang in TODOS {
            let (language, query) = (lang.def().grammar)();
            let q = Query::new(&language, &query).expect("la consulta compila");
            for nombre in q.capture_names() {
                if reconocida(nombre).is_none() && !CAPTURAS_IGNORADAS.contains(nombre) {
                    faltantes.push(format!("{}: @{nombre}", lang.label()));
                }
            }
        }
        faltantes.sort();
        faltantes.dedup();
        assert!(
            faltantes.is_empty(),
            "estas capturas no se traducen a ningún color; agregalas a \
             CAPTURE_NAMES (y a kind_for_capture) o a CAPTURAS_IGNORADAS si \
             es a propósito:\n  {}",
            faltantes.join("\n  ")
        );
    }

    #[test]
    fn el_html_colorea_el_style_como_css_y_el_script_como_javascript() {
        let src = concat!(
            "<style>\n",
            "  body { color: #fff; }\n",
            "</style>\n",
            "<script>\n",
            "  const x = 1;\n",
            "</script>\n",
        );
        let h = LanguageHighlighter::new(&Lang::Html).expect("la gramática compila");
        let spans = h.spans_texto(src);
        let pintado = |texto: &str, kind: HighlightKind| {
            spans
                .iter()
                .any(|(r, k)| *k == kind && src.get(r.clone()) == Some(texto))
        };
        // De la gramática de HTML.
        assert!(pintado("style", HighlightKind::Keyword), "la etiqueta");
        // De la de CSS, inyectada adentro del <style>. El selector sale del
        // mismo color que una etiqueta de HTML, que es lo que es.
        assert!(pintado("body", HighlightKind::Keyword), "el selector de CSS");
        assert!(pintado("color", HighlightKind::Property), "la propiedad CSS");
        // De la de JavaScript, inyectada adentro del <script>.
        assert!(pintado("const", HighlightKind::Keyword), "la palabra clave JS");
        assert!(pintado("1", HighlightKind::Number), "el número JS");
    }

    #[test]
    fn el_cerco_de_markdown_se_colorea_con_el_lenguaje_que_declara() {
        let src = "texto\n\n```rust\nfn main() { let x = 1; }\n```\n";
        let h = LanguageHighlighter::new(&Lang::Markdown).expect("la gramática compila");
        let spans = h.spans_texto(src);
        assert!(
            spans
                .iter()
                .any(|(r, k)| *k == HighlightKind::Keyword && src.get(r.clone()) == Some("fn")),
            "el `fn` de adentro del cerco tenía que salir como palabra clave de Rust"
        );
    }

    #[test]
    fn markdown_sigue_coloreando_lo_de_adentro_de_la_linea() {
        // La pasada inline dejó de estar escrita a mano y pasó a ser una
        // inyección más, igual que el <style> de un HTML. Tiene que dar lo
        // mismo que antes.
        let h = LanguageHighlighter::new(&Lang::Markdown).expect("la gramática compila");
        let src = "un **fuerte**, un *suave* y `codigo`\n";
        let spans = h.spans_texto(src);
        let pintado = |texto: &str, kind: HighlightKind| {
            spans
                .iter()
                .any(|(r, k)| *k == kind && src.get(r.clone()) == Some(texto))
        };
        assert!(pintado("fuerte", HighlightKind::Strong));
        assert!(pintado("suave", HighlightKind::Emphasis));
        assert!(pintado("codigo", HighlightKind::String));
    }

    #[test]
    fn una_inyeccion_de_un_lenguaje_desconocido_no_rompe_nada() {
        // Un cerco que declara un lenguaje que Flint no tiene se colorea como
        // el bloque plano de siempre, no se cae ni deja el resto sin color.
        let src = "```brainfuck\n+++[->+++<]\n```\n\n# Título\n";
        let h = LanguageHighlighter::new(&Lang::Markdown).expect("la gramática compila");
        let spans = h.spans_texto(src);
        assert!(
            spans
                .iter()
                .any(|(r, k)| *k == HighlightKind::Heading && src.get(r.clone()) == Some("Título")),
            "el resto del documento tiene que seguir coloreándose"
        );
    }

    #[test]
    fn la_extension_elige_el_lenguaje() {
        let casos: &[(&str, Lang)] = &[
            ("main.rs", Lang::Rust),
            ("notas.md", Lang::Markdown),
            ("LEEME.markdown", Lang::Markdown),
            ("deploy.sh", Lang::Bash),
            ("main.go", Lang::Go),
            ("app.tsx", Lang::Tsx),
            ("app.ts", Lang::TypeScript),
            ("app.jsx", Lang::JavaScript),
            ("estilo.CSS", Lang::Css),
            ("ci.yml", Lang::Yaml),
            // `.h` es de C, que es el default razonable cuando el mismo
            // encabezado lo puede incluir C o C++.
            ("cosa.h", Lang::C),
            ("cosa.hpp", Lang::Cpp),
            // Sin extensión, el nombre completo alcanza.
            (".bashrc", Lang::Bash),
        ];
        for (nombre, esperado) in casos {
            assert_eq!(
                lang_for_path(Path::new(nombre)),
                Some(*esperado),
                "{nombre} eligió mal"
            );
        }
        assert!(lang_for_path(Path::new("x.desconocida")).is_none());
    }

    #[test]
    fn el_nombre_de_un_cerco_elige_el_lenguaje() {
        let casos: &[(&str, Lang)] = &[
            ("rust", Lang::Rust),
            ("rs", Lang::Rust),
            ("js", Lang::JavaScript),
            ("shell", Lang::Bash),
            ("c++", Lang::Cpp),
            ("golang", Lang::Go),
            ("YAML", Lang::Yaml),
        ];
        for (nombre, esperado) in casos {
            assert_eq!(lang_for_name(nombre), Some(*esperado), "{nombre} eligió mal");
        }
        assert!(lang_for_name("brainfuck").is_none());
    }
}

#[cfg(test)]
mod tests_shebang {
    use super::*;

    fn nombre(l: Option<Lang>) -> Option<&'static str> {
        l.map(|l| l.label())
    }

    #[test]
    fn el_shebang_dice_el_lenguaje_cuando_no_hay_extension() {
        assert_eq!(nombre(lang_for_first_line("#!/usr/bin/python3")), Some("Python"));
        assert_eq!(
            nombre(lang_for_first_line("#!/usr/bin/env python3 -u")),
            Some("Python")
        );
        assert_eq!(nombre(lang_for_first_line("#!/usr/bin/env -S python3")), Some("Python"));
        assert_eq!(nombre(lang_for_first_line("#!/bin/sh")), Some("Shell"));
        assert_eq!(nombre(lang_for_first_line("#!/bin/bash")), Some("Shell"));
        assert_eq!(nombre(lang_for_first_line("#!/usr/bin/env node")), Some("JavaScript"));
        assert_eq!(nombre(lang_for_first_line("#!/usr/bin/env lua5.4")), Some("Lua"));
        assert_eq!(nombre(lang_for_first_line("#!/usr/bin/perl")), None);
        // Sin shebang no hay nada que adivinar.
        assert_eq!(nombre(lang_for_first_line("import os")), None);
        assert_eq!(nombre(lang_for_first_line("")), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resaltado(lang: Lang, src: &str) -> Vec<(String, HighlightKind)> {
        let h = LanguageHighlighter::new(&lang).expect("la gramática compila");
        h.spans_texto(src)
            .into_iter()
            .map(|(r, k)| (src[r].to_string(), k))
            .collect()
    }

    fn tiene(v: &[(String, HighlightKind)], texto: &str, kind: HighlightKind) -> bool {
        v.iter().any(|(t, k)| t == texto && *k == kind)
    }

    #[test]
    fn markdown_resalta_los_bloques() {
        let v = resaltado(Lang::Markdown, "# Título\n\n- item\n");
        assert!(tiene(&v, "Título", HighlightKind::Heading));
    }

    #[test]
    fn markdown_resalta_tambien_lo_de_adentro_de_la_linea() {
        // La segunda pasada, sobre los tramos `inline` del árbol de bloques:
        // sin ella el párrafo entero queda plano.
        let v = resaltado(Lang::Markdown, "un **fuerte**, un *suave* y `codigo`\n");
        assert!(tiene(&v, "fuerte", HighlightKind::Strong));
        assert!(tiene(&v, "suave", HighlightKind::Emphasis));
        assert!(tiene(&v, "codigo", HighlightKind::String));
    }

    #[test]
    fn markdown_resalta_los_links() {
        let v = resaltado(Lang::Markdown, "ver [el manual](https://ejemplo.cl)\n");
        assert!(tiene(&v, "el manual", HighlightKind::Link));
        assert!(tiene(&v, "https://ejemplo.cl", HighlightKind::Link));
    }

    #[test]
    fn los_otros_lenguajes_siguen_igual() {
        let v = resaltado(Lang::Rust, "fn main() { let x = \"hola\"; }\n");
        assert!(tiene(&v, "fn", HighlightKind::Keyword));
        assert!(tiene(&v, "\"hola\"", HighlightKind::String));
    }

    #[test]
    fn la_extension_md_elige_markdown() {
        assert!(matches!(lang_for_path(Path::new("notas.md")), Some(Lang::Markdown)));
        assert!(matches!(lang_for_path(Path::new("LEEME.markdown")), Some(Lang::Markdown)));
        assert!(lang_for_path(Path::new("x.desconocida")).is_none());
    }
}

/// Una muestra corta por lenguaje. Sirve para lo que tiene que correr sobre
/// *todos* (que la consulta compile, que capture algo, y la comparación con
/// el motor viejo) sin que el costo crezca con cada gramática nueva.
#[cfg(test)]
const MUESTRAS_CORTAS: &[(Lang, &str)] = &[
            (Lang::Rust, "fn main() { let x = \"hola\"; }\n"),
            (Lang::Python, "def f(x):\n    return \"hola\"\n"),
            (Lang::Json, "{\"a\": 1}\n"),
            (Lang::Toml, "[x]\na = \"b\"\n"),
            (Lang::Markdown, "# Título\n\ntexto\n"),
            (Lang::Bash, "for f in *.txt; do\n  echo \"$f\"\ndone\n"),
            (Lang::C, "int main(void) { return 0; }\n"),
            (Lang::Cpp, "#include <vector>\nint main() { return 0; }\n"),
            (Lang::Css, "body { color: #fff; }\n"),
            (Lang::Go, "package main\n\nfunc main() {}\n"),
            (Lang::Html, "<p class=\"x\">hola</p>\n"),
            (Lang::Java, "class A { void f() {} }\n"),
            (Lang::JavaScript, "const x = 1;\nfunction f() { return `${x}`; }\n"),
            (Lang::Lua, "local function f(a)\n  return a\nend\n"),
            (Lang::TypeScript, "const x: number = 1;\ninterface A { b: string }\n"),
            (Lang::Tsx, "const A = () => <div className=\"x\">hola</div>;\n"),
            (Lang::Yaml, "clave:\n  - uno\n  - dos\n"),
        
];

/// Muestras de verdad para las dos baterías de abajo: archivos del propio
/// repositorio, que traen todo lo que un ejemplo escrito a mano no tiene
/// (cadenas con acentos, comentarios largos, anidamiento hondo).
#[cfg(test)]
const MUESTRAS: &[(Lang, &str)] = &[
    (Lang::Rust, include_str!("editor.rs")),
    (Lang::Rust, include_str!("markdown.rs")),
    (Lang::Toml, include_str!("../Cargo.toml")),
    (Lang::Toml, include_str!("../config.example.toml")),
    (Lang::Markdown, include_str!("../README.md")),
    (
        Lang::Python,
        "import os\n\n\nclass Perro:\n    \"\"\"Un perro.\"\"\"\n\n    def __init__(self, nombre: str = \"Fido\"):\n        self.nombre = nombre  # el nombre\n\n    def ladrar(self, veces=3):\n        for _ in range(veces):\n            print(f\"{self.nombre}: guau\")\n        return None\n",
    ),
    (
        Lang::Json,
        "{\n  \"nombre\": \"flint\",\n  \"version\": 5,\n  \"activo\": true,\n  \"nada\": null,\n  \"lista\": [1, 2.5, \"tres\"],\n  \"anidado\": {\"a\": {\"b\": [{}]}}\n}\n",
    ),
];

#[cfg(test)]
mod tests_oraculo {
    //! El motor viejo —el de `tree-sitter-highlight`, que reanalizaba el
    //! documento entero en cada pasada— quedó acá como oráculo. La consulta
    //! se recorre ahora a mano para poder reusar el árbol, y esta batería es
    //! la que dice que recorrerla a mano da exactamente lo mismo que antes.
    use super::*;
    use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

    /// La consulta de inyecciones que le hacía falta a la configuración del
    /// motor viejo. Solo Markdown tiene una; para el resto va vacía.
    fn inyecciones(lang: &Lang) -> &'static str {
        match lang {
            Lang::Markdown => tree_sitter_md::INJECTION_QUERY_BLOCK,
            _ => "",
        }
    }

    fn configurar(language: Language, query: &str, inyecciones: &str) -> HighlightConfiguration {
        let mut config =
            HighlightConfiguration::new(language, "flint", query, inyecciones, "").unwrap();
        config.configure(CAPTURE_NAMES);
        config
    }

    fn con(config: &HighlightConfiguration, source: &str) -> Vec<(Range<usize>, HighlightKind)> {
        let mut highlighter = Highlighter::new();
        let mut result = Vec::new();
        let mut stack: Vec<HighlightKind> = Vec::new();
        let Ok(events) = highlighter.highlight(config, source.as_bytes(), None, None, |_| None)
        else {
            return result;
        };
        for event in events.flatten() {
            match event {
                HighlightEvent::HighlightStart(h) => {
                    stack.push(kind_for_capture(CAPTURE_NAMES[h.0]));
                }
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } => {
                    if let Some(&kind) = stack.last() {
                        result.push((start..end, kind));
                    }
                }
            }
        }
        result
    }

    /// El resaltado como lo calculaba el motor viejo, sobre la misma
    /// gramática y la misma consulta que usa el nuevo.
    fn oraculo(lang: &Lang, source: &str) -> Vec<(Range<usize>, HighlightKind)> {
        let (language, query) = (lang.def().grammar)();
        let config = configurar(language, &query, inyecciones(lang));
        con(&config, source)
    }

    #[test]
    fn el_motor_nuevo_da_lo_mismo_que_el_viejo() {
        let todas = MUESTRAS.iter().chain(MUESTRAS_CORTAS.iter());
        for (i, (lang, fuente)) in todas.enumerate() {
            let h = LanguageHighlighter::new(lang).expect("la gramática compila");
            let nuevo = h.spans_base(fuente);
            let viejo = oraculo(lang, fuente);
            assert_eq!(
                nuevo,
                viejo,
                "la muestra {i} ({}) recorre la consulta distinto que el motor viejo",
                lang.label()
            );
        }
    }
}

#[cfg(test)]
mod tests_incremental {
    //! Lo que hay que cuidar del incremental no es que sea rápido sino que dé
    //! *exactamente* lo mismo que rehacerlo todo. Un árbol mal editado no
    //! rompe nada visible: deja colores viejos en lugares que ya cambiaron,
    //! y eso se descubre semanas después. Estas pruebas comparan, después de
    //! cada edición, el resaltado incremental contra uno hecho desde cero.
    use super::*;

    /// El resaltado del texto calculado desde cero, sin nada en caché.
    fn desde_cero(lang: &Lang, texto: &Rope) -> Vec<Vec<(usize, usize, HighlightKind)>> {
        let mut h = LanguageHighlighter::new(lang).expect("la gramática compila");
        let mut ed = Editor::open(None).expect("editor vacío");
        ed.rope = texto.clone();
        ed.highlights_dirty = true;
        refresh(&mut ed, Some(&mut h));
        ed.highlights_by_line
    }

    /// Aplica las ediciones una por una sobre un mismo resaltador —que va
    /// reusando su árbol— y verifica cada paso contra el cálculo completo.
    fn verificar(lang: Lang, fuente: &str, ediciones: &[(usize, usize, &str)]) {
        let mut h = LanguageHighlighter::new(&lang).expect("la gramática compila");
        let mut ed = Editor::open(None).expect("editor vacío");
        ed.rope = Rope::from_str(fuente);
        ed.highlights_dirty = true;
        refresh(&mut ed, Some(&mut h));

        for (paso, &(desde, hasta, texto)) in ediciones.iter().enumerate() {
            let len = ed.rope.len_chars();
            let desde = desde.min(len);
            let hasta = hasta.min(len).max(desde);
            if hasta > desde {
                ed.rope.remove(desde..hasta);
            }
            if !texto.is_empty() {
                ed.rope.insert(desde, texto);
            }
            ed.highlights_dirty = true;
            refresh(&mut ed, Some(&mut h));
            assert_eq!(
                ed.highlights_by_line,
                desde_cero(&lang, &ed.rope),
                "{}: el paso {paso} quedó distinto de recalcular todo",
                lang.label()
            );
        }
    }

    #[test]
    fn escribir_letra_por_letra_no_desalinea_el_arbol() {
        for (lang, fuente) in MUESTRAS {
            // Tipeo en el medio del archivo, carácter por carácter.
            let medio = Rope::from_str(fuente).len_chars() / 2;
            let mut ediciones: Vec<(usize, usize, &str)> = Vec::new();
            for (i, letra) in ["a", "b", "(", ")", " ", "\n", "x"].iter().enumerate() {
                ediciones.push((medio + i, medio + i, letra));
            }
            verificar(*lang, fuente, &ediciones);
        }
    }

    #[test]
    fn el_incremental_anda_en_todas_las_gramaticas() {
        // Barato a propósito: muestras cortas, pero una por lenguaje. Cada
        // gramática reacciona distinto a una edición, y el árbol reusado es
        // el mismo mecanismo para todas.
        for (lang, fuente) in MUESTRAS_CORTAS {
            let medio = Rope::from_str(fuente).len_chars() / 2;
            verificar(
                *lang,
                fuente,
                &[
                    (medio, medio, "x"),
                    (medio, medio, "\n"),
                    (medio, medio + 2, ""),
                    (0, 0, "  "),
                ],
            );
        }
    }

    #[test]
    fn borrar_y_pegar_bloques_enteros() {
        for (lang, fuente) in MUESTRAS {
            let total = Rope::from_str(fuente).len_chars();
            let a = total / 4;
            let b = (total / 4) * 3;
            verificar(
                *lang,
                fuente,
                &[
                    // Borrar la mitad del archivo de una.
                    (a, b, ""),
                    // Volver a meter algo largo en el mismo lugar.
                    (a, a, "\n\nuna linea nueva\notra mas\n\n"),
                    // Vaciarlo del todo.
                    (0, total, ""),
                    // Y rellenarlo otra vez.
                    (0, 0, "hola\nchau\n"),
                ],
            );
        }
    }

    #[test]
    fn abrir_un_comentario_recolorea_hacia_adelante() {
        // El caso que el tramo editado por sí solo no cubre: dos caracteres
        // cambian el color de todo lo que viene después, hasta el cierre.
        let fuente = "fn a() { let x = 1; }\nfn b() { let y = 2; }\n*/\nfn c() {}\n";
        verificar(Lang::Rust, fuente, &[(0, 0, "/*"), (0, 2, "")]);
    }

    #[test]
    fn abrir_una_comilla_recolorea_hacia_adelante() {
        let fuente = "x = 1\ny = 2\nz = \"cierra\"\nw = 3\n";
        verificar(Lang::Python, fuente, &[(4, 4, "\""), (4, 5, "")]);
    }

    #[test]
    fn editar_adentro_de_un_parrafo_de_markdown() {
        // La pasada inline vuelve a correr solo sobre el párrafo tocado; el
        // resto tiene que quedar igual que si se hubiera rehecho entero.
        let fuente = "# Título\n\nun **fuerte** y un *suave*.\n\notro párrafo con `codigo`.\n\n- item\n- otro\n";
        verificar(
            Lang::Markdown,
            fuente,
            &[(13, 13, "muy "), (13, 17, ""), (10, 10, "**")],
        );
    }

    #[test]
    fn acentos_y_emoji_no_parten_un_caracter_al_medio() {
        // El prefijo común de dos textos puede caer en la mitad de una `é`;
        // el corte tiene que retroceder hasta la frontera del carácter.
        let fuente = "// año\nfn f() { let s = \"día 🌞\"; }\n";
        verificar(
            Lang::Rust,
            fuente,
            &[(4, 5, "ñ"), (4, 5, "n"), (25, 25, "🌙"), (25, 26, "")],
        );
    }

    #[test]
    fn el_texto_sin_cambios_no_toca_el_resaltado() {
        let mut h = LanguageHighlighter::new(&Lang::Rust).expect("la gramática compila");
        let mut ed = Editor::open(None).expect("editor vacío");
        ed.rope = Rope::from_str("fn main() { println!(\"hola\"); }\n");
        ed.highlights_dirty = true;
        refresh(&mut ed, Some(&mut h));
        let antes = ed.highlights_by_line.clone();
        ed.highlights_dirty = true;
        refresh(&mut ed, Some(&mut h));
        assert_eq!(antes, ed.highlights_by_line);
    }

    #[test]
    fn el_arbol_en_cache_sirve_para_la_seleccion_estructural() {
        let mut h = LanguageHighlighter::new(&Lang::Rust).expect("la gramática compila");
        let mut ed = Editor::open(None).expect("editor vacío");
        ed.rope = Rope::from_str("fn main() { let x = 1; }\n");
        ed.highlights_dirty = true;
        refresh(&mut ed, Some(&mut h));
        assert!(h.arbol_de(&ed.rope).is_some());
        // Con otro texto la caché no aplica y hay que analizarlo aparte.
        assert!(h.arbol_de(&Rope::from_str("fn otra() {}\n")).is_none());
    }

    #[test]
    fn la_diferencia_describe_el_tramo_que_cambio() {
        let a = Rope::from_str("hola mundo\n");
        let b = Rope::from_str("hola lindo mundo\n");
        assert_eq!(diferencia(&a, &b), Some((5, 5, 11)));
        assert_eq!(diferencia(&b, &a), Some((5, 11, 5)));
        assert_eq!(diferencia(&a, &a), None);
        // Un borrado hasta el final.
        let c = Rope::from_str("hola");
        assert_eq!(diferencia(&a, &c), Some((4, 11, 4)));
    }
}


