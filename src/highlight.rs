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
}

impl Lang {
    pub fn label(&self) -> &'static str {
        match self {
            Lang::Rust => "Rust",
            Lang::Python => "Python",
            Lang::Json => "JSON",
            Lang::Toml => "TOML",
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
}

impl LanguageHighlighter {
    pub fn new(lang: &Lang) -> Option<Self> {
        let (language, query): (tree_sitter::Language, &str) = match lang {
            Lang::Rust => (tree_sitter_rust::LANGUAGE.into(), tree_sitter_rust::HIGHLIGHTS_QUERY),
            Lang::Python => (
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::HIGHLIGHTS_QUERY,
            ),
            Lang::Json => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY),
            Lang::Toml => (
                tree_sitter_toml_ng::LANGUAGE.into(),
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
            ),
        };
        let mut config = HighlightConfiguration::new(language.clone(), "flint", query, "", "").ok()?;
        config.configure(CAPTURE_NAMES);
        Some(LanguageHighlighter { config, language })
    }

    /// Analiza el documento entero y devuelve su árbol de sintaxis, para
    /// selección estructural (expandir la selección al nodo que la contiene).
    pub fn parse(&self, source: &str) -> Option<tree_sitter::Tree> {
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&self.language).ok()?;
        parser.parse(source, None)
    }

    fn highlight_bytes(&self, source: &str) -> Vec<(std::ops::Range<usize>, HighlightKind)> {
        let mut highlighter = Highlighter::new();
        let mut result = Vec::new();
        let mut stack: Vec<HighlightKind> = Vec::new();
        let Ok(events) = highlighter.highlight(&self.config, source.as_bytes(), None, None, |_| None) else {
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
