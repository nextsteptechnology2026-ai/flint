use std::path::Path;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use crate::editor::{Editor, HighlightKind};

/// Lenguajes con resaltado real vía tree-sitter en esta fase. Lo que no está
/// aquí se muestra como texto plano — sigue siendo editable, solo sin color.
#[derive(Clone, Copy)]
pub enum Lang {
    Rust,
    Python,
    Json,
    Toml,
    Markdown,
}

impl Lang {
    pub fn label(&self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::Python => "Python",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
            Lang::Markdown => "Markdown",
        }
    }

    /// Comando del servidor LSP para este lenguaje, si Flint sabe de uno.
    pub fn lsp_command(&self) -> Option<&'static str> {
        match self {
            Lang::Rust => Some("rust-analyzer"),
            _ => None,
        }
    }
}

pub fn lang_for_path(path: &Path) -> Option<Lang> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(Lang::Rust),
        "py" => Some(Lang::Python),
        "json" => Some(Lang::Json),
        "toml" => Some(Lang::Toml),
        "md" | "markdown" => Some(Lang::Markdown),
        _ => None,
    }
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
];

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
    } else if name.starts_with("punctuation") {
        HighlightKind::Punctuation
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

pub struct LanguageHighlighter {
    config: HighlightConfiguration,
    language: tree_sitter::Language,
    /// Gramáticas que la principal puede pedir para tramos de adentro del
    /// documento, por nombre. Markdown es el caso que lo necesita: se parsea
    /// en dos pasadas, una para los bloques (títulos, listas, cercos de
    /// código) y otra *inyectada* para lo de adentro de una línea
    /// (`**negrita**`, `` `código` ``, links). Sin esto se colorean los
    /// bloques pero el texto queda plano.
    injected: Vec<(&'static str, HighlightConfiguration)>,
}

impl LanguageHighlighter {
    pub fn new(lang: &Lang) -> Option<Self> {
        // (gramática, consulta de resaltado, consulta de inyecciones)
        let (language, query, injections): (tree_sitter::Language, &str, &str) = match lang {
            Lang::Rust => (
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                "",
            ),
            Lang::Python => (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::HIGHLIGHTS_QUERY,
                "",
            ),
            Lang::Json => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY, ""),
            Lang::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
                "",
            ),
            Lang::Markdown => (
                tree_sitter_md::LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                tree_sitter_md::INJECTION_QUERY_BLOCK,
            ),
        };
        let mut config =
            HighlightConfiguration::new(language.clone(), "flint", query, injections, "").ok()?;
        config.configure(CAPTURE_NAMES);

        let mut injected = Vec::new();
        if matches!(lang, Lang::Markdown) {
            let mut inline = HighlightConfiguration::new(
                tree_sitter_md::INLINE_LANGUAGE.into(),
                "markdown_inline",
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                tree_sitter_md::INJECTION_QUERY_INLINE,
                "",
            )
            .ok()?;
            inline.configure(CAPTURE_NAMES);
            injected.push(("markdown_inline", inline));
        }

        Some(LanguageHighlighter { config, language, injected })
    }

    /// Analiza el documento entero y devuelve su árbol de sintaxis, para
    /// selección estructural (expandir la selección al nodo que la contiene).
    pub fn parse(&self, source: &str) -> Option<tree_sitter::Tree> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&self.language).ok()?;
        parser.parse(source, None)
    }

    /// Todo el documento con la gramática principal, más —si el lenguaje lo
    /// necesita— una segunda pasada sobre los tramos que se parsean con otra
    /// gramática. Markdown es hoy el único caso: el árbol de bloques marca
    /// dónde hay texto de una línea (`inline`) y ese tramo se vuelve a
    /// analizar con la gramática inline, que es la que sabe de `**negrita**`,
    /// `` `código` `` y links.
    ///
    /// Se hace explícito, y no con el mecanismo de inyecciones de
    /// `tree-sitter-highlight`, porque ahí la capa inyectada no llegaba a
    /// emitir ningún evento: el callback se llamaba y devolvía la gramática
    /// correcta, pero el resultado nunca aparecía en el flujo. Dos pasadas a
    /// mano son treinta líneas que se pueden leer y probar.
    ///
    /// El orden importa: lo inline va después, y al pintar gana lo último,
    /// que es lo que se quiere (el detalle por encima del bloque).
    fn highlight_bytes(&self, source: &str) -> Vec<(std::ops::Range<usize>, HighlightKind)> {
        let mut result = self.highlight_with(&self.config, source);
        if let Some((_, inline_config)) = self.injected.first() {
            for range in self.inline_ranges(source) {
                let offset = range.start;
                let fragment = &source[range];
                result.extend(
                    self.highlight_with(inline_config, fragment)
                        .into_iter()
                        .map(|(r, k)| (r.start + offset..r.end + offset, k)),
                );
            }
        }
        result
    }

    /// Los tramos de texto que el árbol de bloques marca como `inline`.
    fn inline_ranges(&self, source: &str) -> Vec<std::ops::Range<usize>> {
        let Some(tree) = self.parse(source) else {
            return Vec::new();
        };
        let mut ranges = Vec::new();
        let mut pila = vec![tree.root_node()];
        while let Some(node) = pila.pop() {
            if node.kind() == "inline" {
                ranges.push(node.start_byte()..node.end_byte().min(source.len()));
                continue;
            }
            let mut cursor = node.walk();
            pila.extend(node.children(&mut cursor));
        }
        ranges
    }

    fn highlight_with(
        &self,
        config: &HighlightConfiguration,
        source: &str,
    ) -> Vec<(std::ops::Range<usize>, HighlightKind)> {
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
}

/// Recalcula `ed.highlights_by_line` a partir del contenido actual del buffer.
/// Solo hace trabajo real si `ed.highlights_dirty` está encendido.
pub fn refresh(ed: &mut Editor, highlighter: Option<&LanguageHighlighter>) {
    if !ed.highlights_dirty {
        return;
    }
    let line_count = ed.line_count().max(1);
    let mut by_line: Vec<Vec<(usize, usize, HighlightKind)>> = vec![Vec::new(); line_count];

    if let Some(highlighter) = highlighter {
        let text = ed.rope.to_string();
        let len_bytes = ed.rope.len_bytes();
        let len_chars = ed.rope.len_chars();

        for (byte_range, kind) in highlighter.highlight_bytes(&text) {
            let start_char = ed.rope.byte_to_char(byte_range.start.min(len_bytes));
            let end_char = ed.rope.byte_to_char(byte_range.end.min(len_bytes));
            if end_char <= start_char {
                continue;
            }
            let start_line = ed.rope.char_to_line(start_char.min(len_chars));
            let end_line = ed
                .rope
                .char_to_line((end_char - 1).min(len_chars.saturating_sub(1)));

            for line in start_line..=end_line.min(by_line.len().saturating_sub(1)) {
                let line_start = ed.rope.line_to_char(line);
                let line_end = line_start + ed.line_char_len(line);
                let s = start_char.max(line_start);
                let e = end_char.min(line_end);
                if s < e {
                    by_line[line].push((s - line_start, e - line_start, kind));
                }
            }
        }
    }

    ed.highlights_by_line = by_line;
    ed.highlights_dirty = false;
}

/// Direccionamiento estructural: dado un rango de bytes seleccionado, encuentra
/// el nodo del árbol de sintaxis más pequeño que lo contiene. Si la selección
/// ya coincide exactamente con un nodo, sube a su nodo padre — así presionar la
/// tecla repetidas veces va ampliando la selección un nivel del árbol por vez.
pub fn expand_selection(tree: &tree_sitter::Tree, start_byte: usize, end_byte: usize) -> Option<(usize, usize)> {
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
mod tests {
    use super::*;

    fn resaltado(lang: Lang, src: &str) -> Vec<(String, HighlightKind)> {
        let h = LanguageHighlighter::new(&lang).expect("la gramática compila");
        h.highlight_bytes(src)
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
