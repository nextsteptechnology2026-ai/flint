use std::path::{Path, PathBuf};

use ratatui::style::Color;
use serde::Deserialize;

/// Estilo del cursor de la terminal (el real, no una selección resaltada) —
/// ver `crossterm::cursor::SetCursorStyle`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

pub struct Theme {
    pub bar_bg: Color,
    pub bar_fg: Color,
    pub text_fg: Color,
    pub dim: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub secondary_bg: Color,
    pub secondary_fg: Color,
    pub popup_bg: Color,
    pub error: Color,
    pub warning: Color,
    pub hint: Color,

    pub syn_keyword: Color,
    pub syn_function: Color,
    pub syn_type: Color,
    pub syn_string: Color,
    pub syn_comment: Color,
    pub syn_number: Color,
    pub syn_constant: Color,
    pub syn_operator: Color,
    pub syn_punctuation: Color,
    pub syn_property: Color,
    pub syn_attribute: Color,

    /// Si es true, la barra de título/ayuda/mensaje no pintan un fondo
    /// propio y dejan pasar el fondo real (y su transparencia, si la
    /// terminal la soporta) de tu emulador. El área de texto ya se comporta
    /// así siempre — nunca pintó su propio fondo — así que esto solo decide
    /// las barras.
    pub transparent_bars: bool,
    pub cursor_shape: CursorShape,
    pub cursor_blink: bool,
    /// Cada cuántas columnas cae una parada de tabulación al dibujar un `\t`.
    /// Es presentación pura (por eso vive acá y no en el buffer): cambiarlo
    /// no toca el archivo, solo cuánto se estira cada tabulador en pantalla.
    pub tab_width: usize,
}

impl Default for Theme {
    /// La paleta amber/terminal con la que arrancó Flint, tal cual estaba
    /// escrita directamente en `ui.rs` antes de que existiera este archivo.
    fn default() -> Self {
        Theme {
            bar_bg: Color::Rgb(20, 23, 27),
            bar_fg: Color::Rgb(242, 169, 59),
            text_fg: Color::Rgb(210, 208, 196),
            dim: Color::Rgb(120, 124, 112),
            selection_bg: Color::Rgb(58, 44, 20),
            selection_fg: Color::Rgb(233, 230, 218),
            secondary_bg: Color::Rgb(95, 214, 201),
            secondary_fg: Color::Rgb(20, 23, 27),
            popup_bg: Color::Rgb(28, 32, 37),
            error: Color::Rgb(224, 108, 90),
            warning: Color::Rgb(230, 180, 90),
            hint: Color::Rgb(120, 150, 200),

            syn_keyword: Color::Rgb(230, 150, 60),
            syn_function: Color::Rgb(95, 214, 201),
            syn_type: Color::Rgb(230, 196, 120),
            syn_string: Color::Rgb(150, 196, 120),
            syn_comment: Color::Rgb(120, 124, 112),
            syn_number: Color::Rgb(196, 140, 214),
            syn_constant: Color::Rgb(196, 140, 214),
            syn_operator: Color::Rgb(160, 164, 150),
            syn_punctuation: Color::Rgb(160, 164, 150),
            syn_property: Color::Rgb(210, 176, 110),
            syn_attribute: Color::Rgb(210, 176, 110),

            transparent_bars: false,
            cursor_shape: CursorShape::Block,
            cursor_blink: true,
            tab_width: crate::editor::DEFAULT_TAB_WIDTH,
        }
    }
}

/// El mismo archivo, pero como lo entiende `toml`/`serde`: todo opcional
/// (falta un campo → se conserva el valor por defecto en esa entrada) y los
/// colores como strings `"#rrggbb"` en vez de `Color` directamente.
#[derive(Deserialize, Default)]
struct RawTheme {
    #[serde(default)]
    colors: RawColors,
    #[serde(default)]
    syntax: RawSyntax,
    #[serde(default)]
    appearance: RawAppearance,
}

#[derive(Deserialize, Default)]
struct RawColors {
    bar_bg: Option<String>,
    bar_fg: Option<String>,
    text_fg: Option<String>,
    dim: Option<String>,
    selection_bg: Option<String>,
    selection_fg: Option<String>,
    secondary_bg: Option<String>,
    secondary_fg: Option<String>,
    popup_bg: Option<String>,
    error: Option<String>,
    warning: Option<String>,
    hint: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawSyntax {
    keyword: Option<String>,
    function: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
    string: Option<String>,
    comment: Option<String>,
    number: Option<String>,
    constant: Option<String>,
    operator: Option<String>,
    punctuation: Option<String>,
    property: Option<String>,
    attribute: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawAppearance {
    transparent_bars: Option<bool>,
    cursor_style: Option<String>,
    cursor_blink: Option<bool>,
    tab_width: Option<usize>,
}

/// Ancho de tabulación máximo aceptado: más que esto no es una preferencia,
/// es una línea entera de sangría por cada `\t`.
const MAX_TAB_WIDTH: usize = 16;

fn parse_hex_color(s: &str) -> Option<Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

impl Theme {
    /// Ruta por defecto del tema: `~/.config/flint/theme.toml`.
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(PathBuf::from(home).join(".config/flint/theme.toml"))
    }

    /// Carga un tema desde `path`. Devuelve el tema resultante (empezando
    /// del default y aplicando encima lo que el archivo sí trae) y una lista
    /// de avisos — un color que no se pudo interpretar, por ejemplo, no
    /// aborta la carga: esa entrada puntual se queda en su valor por
    /// defecto y el resto del archivo se sigue aplicando igual.
    pub fn load(path: &Path) -> (Theme, Vec<String>) {
        let mut theme = Theme::default();
        let mut warnings = Vec::new();

        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("No se pudo leer {}: {e}", path.display()));
                return (theme, warnings);
            }
        };
        let raw: RawTheme = match toml::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                warnings.push(format!("{}: {e}", path.display()));
                return (theme, warnings);
            }
        };

        let mut set = |field: &mut Color, name: &str, value: &Option<String>| {
            if let Some(s) = value {
                match parse_hex_color(s) {
                    Some(c) => *field = c,
                    None => warnings.push(format!(
                        "color inválido en {name} = \"{s}\" (usá el formato \"#rrggbb\")"
                    )),
                }
            }
        };

        set(&mut theme.bar_bg, "colors.bar_bg", &raw.colors.bar_bg);
        set(&mut theme.bar_fg, "colors.bar_fg", &raw.colors.bar_fg);
        set(&mut theme.text_fg, "colors.text_fg", &raw.colors.text_fg);
        set(&mut theme.dim, "colors.dim", &raw.colors.dim);
        set(&mut theme.selection_bg, "colors.selection_bg", &raw.colors.selection_bg);
        set(&mut theme.selection_fg, "colors.selection_fg", &raw.colors.selection_fg);
        set(&mut theme.secondary_bg, "colors.secondary_bg", &raw.colors.secondary_bg);
        set(&mut theme.secondary_fg, "colors.secondary_fg", &raw.colors.secondary_fg);
        set(&mut theme.popup_bg, "colors.popup_bg", &raw.colors.popup_bg);
        set(&mut theme.error, "colors.error", &raw.colors.error);
        set(&mut theme.warning, "colors.warning", &raw.colors.warning);
        set(&mut theme.hint, "colors.hint", &raw.colors.hint);

        set(&mut theme.syn_keyword, "syntax.keyword", &raw.syntax.keyword);
        set(&mut theme.syn_function, "syntax.function", &raw.syntax.function);
        set(&mut theme.syn_type, "syntax.type", &raw.syntax.type_);
        set(&mut theme.syn_string, "syntax.string", &raw.syntax.string);
        set(&mut theme.syn_comment, "syntax.comment", &raw.syntax.comment);
        set(&mut theme.syn_number, "syntax.number", &raw.syntax.number);
        set(&mut theme.syn_constant, "syntax.constant", &raw.syntax.constant);
        set(&mut theme.syn_operator, "syntax.operator", &raw.syntax.operator);
        set(&mut theme.syn_punctuation, "syntax.punctuation", &raw.syntax.punctuation);
        set(&mut theme.syn_property, "syntax.property", &raw.syntax.property);
        set(&mut theme.syn_attribute, "syntax.attribute", &raw.syntax.attribute);

        if let Some(t) = raw.appearance.transparent_bars {
            theme.transparent_bars = t;
        }
        if let Some(b) = raw.appearance.cursor_blink {
            theme.cursor_blink = b;
        }
        // Un 0 haría que un `\t` no ocupe nada (justo el bug que este ancho
        // existe para arreglar), así que un valor fuera de rango se avisa y
        // se queda en el default en vez de aplicarse a medias.
        if let Some(w) = raw.appearance.tab_width {
            if (1..=MAX_TAB_WIDTH).contains(&w) {
                theme.tab_width = w;
            } else {
                warnings.push(format!(
                    "appearance.tab_width = {w} fuera de rango (usá un número entre 1 y {MAX_TAB_WIDTH})"
                ));
            }
        }
        if let Some(shape) = &raw.appearance.cursor_style {
            match shape.to_ascii_lowercase().as_str() {
                "block" => theme.cursor_shape = CursorShape::Block,
                "bar" | "beam" | "line" => theme.cursor_shape = CursorShape::Bar,
                "underline" | "underscore" => theme.cursor_shape = CursorShape::Underline,
                other => warnings.push(format!(
                    "appearance.cursor_style = \"{other}\" desconocido (usá block, bar o underline)"
                )),
            }
        }

        (theme, warnings)
    }

    /// El estilo de fondo de una barra: sólido, o transparente si
    /// `transparent_bars` está activo.
    pub fn bar_style(&self) -> ratatui::style::Style {
        let s = ratatui::style::Style::default().fg(self.bar_fg);
        if self.transparent_bars {
            s
        } else {
            s.bg(self.bar_bg)
        }
    }

    pub fn crossterm_cursor_style(&self) -> crossterm::cursor::SetCursorStyle {
        use crossterm::cursor::SetCursorStyle as S;
        match (self.cursor_shape, self.cursor_blink) {
            (CursorShape::Block, true) => S::BlinkingBlock,
            (CursorShape::Block, false) => S::SteadyBlock,
            (CursorShape::Bar, true) => S::BlinkingBar,
            (CursorShape::Bar, false) => S::SteadyBar,
            (CursorShape::Underline, true) => S::BlinkingUnderScore,
            (CursorShape::Underline, false) => S::SteadyUnderScore,
        }
    }
}
