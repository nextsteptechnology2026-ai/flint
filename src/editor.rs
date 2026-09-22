use regex::Regex;
use ropey::Rope;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthChar;

/// Ediciones del mismo tipo hechas dentro de esta ventana de tiempo se agrupan
/// en un solo paso de deshacer (igual que "escribir una palabra" cuenta como uno).
const GROUP_TIMEOUT: Duration = Duration::from_millis(700);
const MAX_UNDO: usize = 500;
/// Límite defensivo al buscar ocurrencias para "seleccionar la siguiente
/// aparición" — de sobra para el tamaño de prototipo.
const MAX_OCCURRENCES: usize = 500;
/// Ancho por defecto de una parada de tabulación, en columnas de pantalla —
/// la convención de la mayoría de los editores de terminal.
pub const DEFAULT_TAB_WIDTH: usize = 4;

/// Cuántas columnas de pantalla ocupa `c` empezando a dibujarse en la columna
/// `at`. Tres casos:
///
/// - el tabulador se estira hasta la próxima parada de tabulación, así que su
///   ancho depende de dónde empieza (de 1 a `tab_width`);
/// - los caracteres anchos (CJK, emoji) ocupan dos columnas;
/// - las marcas combinantes (un acento que se pinta sobre la letra anterior)
///   ocupan cero, igual que los caracteres de control.
///
/// El ancho lo decide `unicode-width`, la misma tabla que usa ratatui para
/// medir lo que dibuja: usar otra regla es justamente lo que desalinea el
/// cursor del texto.
pub fn char_display_width(c: char, at: usize, tab_width: usize) -> usize {
    if c == '\t' {
        tab_width - (at % tab_width.max(1))
    } else {
        UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

struct UndoEntry {
    rope: Rope,
    selections: Vec<Selection>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

/// Una selección: `anchor` es el punto fijo, `cursor` el que se mueve.
/// `anchor == cursor` significa "sin rango, solo un cursor en ese punto".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Selection {
    pub anchor: Position,
    pub cursor: Position,
}

impl Selection {
    fn point(pos: Position) -> Self {
        Selection {
            anchor: pos,
            cursor: pos,
        }
    }
}

/// Categoría de resaltado de sintaxis; el color concreto lo decide la capa de UI.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HighlightKind {
    Keyword,
    Function,
    Type,
    String,
    Comment,
    Number,
    Constant,
    Operator,
    Punctuation,
    Variable,
    Property,
    Attribute,
    /// Las cuatro siguientes son de texto con formato (Markdown), no de
    /// código: no se distinguen por color sino por *forma* — un título en
    /// negrita, un link subrayado. Ver `highlight_style` en `ui.rs`.
    Heading,
    Strong,
    Emphasis,
    Link,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl Severity {
    fn rank(self) -> u8 {
        match self {
            Severity::Error => 3,
            Severity::Warning => 2,
            Severity::Info => 1,
            Severity::Hint => 0,
        }
    }
}

/// El rango exacto que reporta el servidor (`start_col`..`end_col` en
/// `line`, o hasta `end_line` si cruza más de una) — no solo la línea. La UI
/// usa esto para subrayar el tramo preciso, además de marcar el renglón en
/// el margen.
#[derive(Clone)]
pub struct Diagnostic {
    pub line: usize,
    pub start_col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub severity: Severity,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct CompletionEntry {
    pub label: String,
    pub detail: Option<String>,
    /// Texto a insertar si se elige esta entrada, cuando el servidor lo da y
    /// no es un snippet (`$0`, `${1:nombre}`...) — Flint no expande snippets
    /// todavía, así que en ese caso se usa `label` en cambio.
    pub insert_text: Option<String>,
    /// El rango de `textEdit` tal como lo manda el servidor: (línea,
    /// columna UTF-16) de inicio y de fin, medido sobre el texto del momento
    /// en que se pidió el autocompletado.
    pub lsp_range: Option<((usize, usize), (usize, usize))>,
    /// Ese mismo rango pasado a lo que Flint necesita al aceptar: la columna
    /// (en caracteres) desde la que se borra, y cuántos caracteres después
    /// del cursor se borran también. `None` = lo de siempre: desde el
    /// comienzo de la palabra hasta el cursor.
    pub reemplazo: Option<(usize, usize)>,
}

/// La capa opcional de edición modal (selección→acción, estilo Kakoune/Helix),
/// activable con F2. Por defecto el editor arranca en `Direct`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Direct,
    ModalNormal,
    ModalInsert,
}

pub enum PromptKind {
    SaveAs { then_quit: bool },
    Search,
    ReplaceSearch,
    ReplaceWith { search: String },
    /// Igual que `Search`, pero el texto se interpreta como expresión
    /// regular (sintaxis del crate `regex`), no texto literal.
    SearchRegex,
    /// Igual que `ReplaceSearch`, pero para el flujo con regex.
    ReplaceRegexSearch,
    /// Igual que `ReplaceWith`, pero `pattern` es la regex ya validada y el
    /// reemplazo puede referirse a grupos capturados (`$1`, `${nombre}`).
    ReplaceRegexWith { pattern: String },
    QuitConfirm,
    /// Ruta a abrir en un buffer nuevo (Ctrl+O). Se resuelve en `App`, no acá
    /// — abrir un archivo implica sumar un `Buffer` entero, no solo tocar el
    /// `Editor` activo.
    OpenFile,
    /// Número de línea al que saltar, contado desde 1 como lo cuenta la
    /// barra de estado (y como lo escribe cualquier compilador).
    GotoLine,
    /// El archivo cambió en disco desde que se abrió: confirmar antes de
    /// pisar lo que haya escrito el otro proceso.
    OverwriteConfirm { then_quit: bool },
    /// El nombre nuevo para el símbolo bajo el cursor. Se resuelve en `App`,
    /// no acá: el servidor de lenguaje puede contestar cambios en varios
    /// archivos, no solo en este buffer.
    Rename { palabra: String },
    /// El texto que reemplaza a `consulta` en `rutas`, los archivos del
    /// proyecto donde aparece. Se resuelve en `App`: abre los archivos.
    ReplaceProject { consulta: String, regex: bool, rutas: Vec<std::path::PathBuf> },
}

pub enum Mode {
    Editing,
    Prompt {
        kind: PromptKind,
        buffer: String,
        label: String,
    },
    Completion {
        items: Vec<CompletionEntry>,
        selected: usize,
        /// Dónde estaba el cursor cuando se abrió el popup — al aceptar una
        /// entrada, se borra todo lo que se haya tipeado desde acá y se
        /// inserta el texto elegido, así da igual qué tan distinto es el
        /// filtro tipeado del texto final (mayúsculas, coincidencia difusa,
        /// lo que sea): siempre queda exactamente lo que el servidor ofreció.
        trigger: Position,
        /// Lo tipeado desde que se abrió el popup, para filtrar la lista en
        /// caliente (y para saber cuánto borrar al aceptar).
        prefix: String,
    },
    /// Paleta de comandos (Ctrl+P): la lista completa y el filtrado difuso
    /// viven en `App`, acá solo el texto que se está escribiendo y cuál está
    /// resaltado.
    Palette { query: String, selected: usize },
    /// Buscador de archivos del proyecto (Ctrl+T). Se dibuja igual que la
    /// paleta y filtra igual; lo que cambia es la lista de abajo (rutas en
    /// vez de comandos) y qué hace Enter (abrir un buffer).
    FilePicker { query: String, selected: usize },
    /// Buscar texto en todo el proyecto (Ctrl+N). Las coincidencias viven
    /// en `App`, igual que la lista del buscador de archivos; acá va lo que
    /// se escribe, cuál está resaltada y el resumen de la última búsqueda
    /// ("12 en 3 archivos") para mostrarlo al lado.
    ProjectSearch { query: String, selected: usize, resumen: String, regex: bool },
    /// La ventana de hover (F1) abierta junto al cursor. Las líneas ya
    /// dibujadas viven en `App`; acá solo cuánto se desplazó.
    Hover { offset: usize },
}

/// Qué secuencia de caracteres separa líneas en este buffer — detectada al
/// abrir el archivo, para que una línea *nueva* (`Enter`) siga la misma
/// convención que las que ya estaban ahí, en vez de siempre meter un `\n`
/// suelto y terminar con el archivo en una mezcla de los dos estilos.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }

    /// Mira el primer salto de línea del texto para decidir; sin ninguno
    /// (archivo nuevo o de una sola línea), el default es LF.
    fn detect(content: &str) -> LineEnding {
        match content.find('\n') {
            Some(0) => LineEnding::Lf,
            Some(byte) if content.as_bytes()[byte - 1] == b'\r' => LineEnding::CrLf,
            _ => LineEnding::Lf,
        }
    }
}

pub struct Editor {
    pub rope: Rope,
    pub filename: Option<PathBuf>,
    pub line_ending: LineEnding,
    /// Cursor y selección primarios — el único par que existía antes del
    /// multi-cursor, y el que sigue mandando en búsqueda, LSP, scroll, etc.
    pub cursor: Position,
    pub selection_anchor: Option<Position>,
    /// Selecciones adicionales (multi-cursor). Vacío la mayor parte del tiempo.
    pub secondary: Vec<Selection>,
    pub row_offset: usize,
    /// Cuántas filas visuales de la *primera* línea en pantalla quedan
    /// arriba del borde. Solo se usa con ajuste de línea, y casi siempre es
    /// 0: hace falta cuando una sola línea lógica ocupa más filas que la
    /// pantalla entera, donde desplazarse de a líneas enteras dejaría partes
    /// de esa línea imposibles de ver.
    pub row_sub_offset: usize,
    pub col_offset: usize,
    pub dirty: bool,
    /// Se guardó y todavía no se avisó a los plugins. Lo marca `save`, que es
    /// por donde pasan todos los caminos de guardar (Ctrl+S, confirmar que
    /// se pisa el disco, "Guardar como"), y lo levanta el bucle principal.
    pub recien_guardado: bool,
    pub status: String,
    pub mode: Mode,
    pub last_search: Option<String>,
    pub should_quit: bool,
    pub highlights_by_line: Vec<Vec<(usize, usize, HighlightKind)>>,
    pub highlights_dirty: bool,
    /// Se incrementa en cada mutación real del buffer; sirve para que el
    /// cliente LSP sepa cuándo el contenido enviado quedó desactualizado.
    pub content_version: u64,
    pub diagnostics: Vec<Diagnostic>,
    pub layer: Layer,
    /// Ajuste de línea: las líneas largas se parten en varias filas de
    /// pantalla en vez de desplazarse horizontalmente. Apagado por defecto
    /// (`col_offset`/scroll horizontal, el comportamiento de siempre).
    pub wrap: bool,
    /// Vista previa de Markdown: se muestra el documento formateado, de solo
    /// lectura, en vez del texto fuente. Solo tiene sentido —y solo se
    /// enciende— en un archivo Markdown.
    pub preview: bool,
    /// Primera fila de la vista previa que se muestra. Es un espacio de
    /// filas propio, distinto del `row_offset` del texto fuente: el
    /// documento renderizado no tiene las mismas líneas que el original.
    pub preview_offset: usize,
    /// Registro interno de copiar/pegar propio de Flint. Es el que se usa
    /// para pegar cuando no hay portapapeles del sistema disponible, y
    /// siempre recibe una copia de lo que se copia (ver `clipboard.rs`).
    pub register: Option<String>,
    /// Cada cuántas columnas cae una parada de tabulación al dibujar un `\t`.
    /// Es solo presentación: en el buffer el tabulador sigue siendo un único
    /// carácter, igual que en el archivo.
    pub tab_width: usize,
    /// Si al indentar se escriben `tab_width` espacios en vez de un `\t`.
    /// Esto sí cambia el archivo, al revés que `tab_width` — por eso son dos
    /// cosas separadas y no una sola.
    pub indent_with_spaces: bool,
    /// Si al guardar se recortan los espacios del final de cada línea.
    pub trim_on_save: bool,
    /// Si antes de guardar se le pide al servidor de lenguaje que formatee.
    /// Lo resuelve `App`, que es quien habla con el servidor: `save` no lo
    /// mira.
    pub format_on_save: bool,
    /// Si escribir `(`, `[`, `{`, `"` o `'` agrega también el de cierre.
    pub auto_close: bool,
    /// Si una línea terminada en `:` abre un bloque, como en Python. En un
    /// lenguaje con llaves no, y ahí un `:` al final es otra cosa.
    pub indent_after_colon: bool,
    /// La fecha de modificación que tenía el archivo la última vez que Flint
    /// lo leyó o lo escribió. Sirve para darse cuenta de que otro proceso lo
    /// cambió mientras estaba abierto, antes de pisarlo.
    pub disk_mtime: Option<SystemTime>,
    /// Cuándo se escribió el último respaldo, para no escribir uno por tecla.
    pub last_backup: Option<Instant>,
    undo_stack: Vec<UndoEntry>,
    redo_stack: Vec<UndoEntry>,
    last_edit_kind: Option<EditKind>,
    last_edit_at: Option<Instant>,
    /// Ediciones de un solo cursor, sin sincronizar todavía con el LSP, en
    /// el orden en que pasaron de verdad — ver `edit_all_selections` y
    /// `take_lsp_sync_plan`.
    pending_lsp_edits: Vec<LspEdit>,
    /// Se prendió una edición que no se puede representar como una lista de
    /// deltas incrementales válida para LSP (multi-cursor, deshacer/rehacer,
    /// reemplazar todo) — la próxima sincronización manda el documento
    /// completo en vez de deltas, y así se resetea.
    needs_full_lsp_sync: bool,
}

/// Una edición puntual, en términos de posición (línea/columna) — lo que
/// necesita `textDocument/didChange` para un delta incremental real, en vez
/// de mandar el documento entero en cada cambio.
#[derive(Clone)]
/// Las columnas ya están en unidades UTF-16 (lo que pide LSP), calculadas en
/// el momento mismo de la edición contra el rope de ese instante — hacerlo
/// más tarde, con el rope ya en otro estado, daría un número distinto si
/// alguna edición posterior en la misma línea cambió el ancho UTF-16 de lo
/// que hay *antes* de esta columna (un emoji insertado a la izquierda, por
/// ejemplo).
pub struct LspEdit {
    pub start_line: usize,
    pub start_col_utf16: usize,
    pub end_line: usize,
    pub end_col_utf16: usize,
    pub text: String,
}

/// Qué hay que mandarle al servidor la próxima vez que se sincronice este
/// buffer — ver `Editor::take_lsp_sync_plan`.
pub enum LspSyncPlan {
    /// Nada pendiente, no hace falta mandar nada.
    None,
    /// Deltas incrementales, en orden — de sobra para el caso común
    /// (escribir, borrar, un cursor).
    Incremental(Vec<LspEdit>),
    /// Mandar el documento completo — deshacer/rehacer, reemplazar todo, o
    /// una edición multi-cursor que no se puede representar como secuencia
    /// de deltas.
    Full,
}

/// La fecha de modificación de un archivo, o `None` si no existe o no se
/// puede leer.
fn mtime_de(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Junta las ediciones de un multi-cursor que se pisan. Si dos rangos a
/// borrar se superponen (una selección y un cursor adentro de ella, o dos
/// selecciones que se cruzaron al extenderlas) o empiezan en el mismo lugar
/// (dos cursores apilados), aplicarlos por separado sobre el mismo texto
/// borraría de más: el segundo usa índices que el primero ya corrió. Se
/// reemplazan por una sola edición sobre la unión, con el texto de una sola
/// de ellas, y queda el índice menor para que la primaria no se pierda.
/// Rangos que solo se tocan en un borde (`0..1` y `1..2`) no se pisan y se
/// dejan como están.
fn fundir_ediciones(mut planned: Vec<(usize, usize, usize, String)>) -> Vec<(usize, usize, usize, String)> {
    planned.sort_by_key(|&(_, inicio, fin, _)| (inicio, fin));
    let mut fundidas: Vec<(usize, usize, usize, String)> = Vec::with_capacity(planned.len());
    for edicion in planned {
        if let Some(anterior) = fundidas.last_mut()
            && (edicion.1 < anterior.2 || edicion.1 == anterior.1)
        {
            anterior.0 = anterior.0.min(edicion.0);
            anterior.2 = anterior.2.max(edicion.2);
            continue;
        }
        fundidas.push(edicion);
    }
    fundidas
}

/// Las expresiones regulares de buscar y reemplazar se compilan en modo
/// multilínea: en un editor, `^` y `$` son el principio y el fin de cada
/// línea, no los del archivo entero. Sin esto `^fn` solo encontraba algo si
/// el archivo empezaba con `fn`.
fn regex_de_editor(pattern: &str) -> Result<Regex, String> {
    regex::RegexBuilder::new(pattern)
        .multi_line(true)
        .build()
        .map_err(|e| e.to_string())
}

impl Editor {
    pub fn open(path: Option<PathBuf>) -> io::Result<Editor> {
        let disk_mtime = path.as_ref().and_then(|p| mtime_de(p));
        let (rope, filename, status, line_ending) = match path {
            Some(p) => {
                if p.exists() {
                    let content = fs::read_to_string(&p)?;
                    let le = LineEnding::detect(&content);
                    (Rope::from_str(&content), Some(p), "Listo".to_string(), le)
                } else {
                    (Rope::new(), Some(p), "[Archivo nuevo]".to_string(), LineEnding::Lf)
                }
            }
            None => (Rope::new(), None, "[Sin nombre]".to_string(), LineEnding::Lf),
        };
        Ok(Editor {
            rope,
            filename,
            line_ending,
            cursor: Position { line: 0, col: 0 },
            selection_anchor: None,
            secondary: Vec::new(),
            row_offset: 0,
            row_sub_offset: 0,
            col_offset: 0,
            dirty: false,
            recien_guardado: false,
            status,
            mode: Mode::Editing,
            last_search: None,
            should_quit: false,
            highlights_by_line: Vec::new(),
            highlights_dirty: true,
            content_version: 0,
            diagnostics: Vec::new(),
            layer: Layer::Direct,
            wrap: false,
            preview: false,
            preview_offset: 0,
            register: None,
            tab_width: DEFAULT_TAB_WIDTH,
            indent_with_spaces: false,
            trim_on_save: false,
            format_on_save: false,
            auto_close: true,
            indent_after_colon: false,
            disk_mtime,
            last_backup: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_kind: None,
            last_edit_at: None,
            pending_lsp_edits: Vec::new(),
            needs_full_lsp_sync: false,
        })
    }

    /// Qué mandarle al servidor en la próxima sincronización, y limpia el
    /// estado pendiente — se llama una sola vez por sincronización real.
    pub fn take_lsp_sync_plan(&mut self) -> LspSyncPlan {
        if self.needs_full_lsp_sync {
            self.needs_full_lsp_sync = false;
            self.pending_lsp_edits.clear();
            return LspSyncPlan::Full;
        }
        if self.pending_lsp_edits.is_empty() {
            return LspSyncPlan::None;
        }
        LspSyncPlan::Incremental(std::mem::take(&mut self.pending_lsp_edits))
    }

    /// Severidad más alta entre los diagnósticos que caen en esta línea, si hay alguno.
    pub fn diagnostic_severity_for_line(&self, line: usize) -> Option<Severity> {
        self.diagnostics
            .iter()
            .filter(|d| d.line == line)
            .map(|d| d.severity)
            .max_by_key(|s| s.rank())
    }

    /// Mueve el cursor primario al siguiente diagnóstico (dando la vuelta) y
    /// devuelve sus mensajes. Descarta los cursores extra: es un salto a un
    /// lugar concreto, no una operación multi-selección.
    pub fn jump_to_next_diagnostic(&mut self) -> Option<String> {
        if self.diagnostics.is_empty() {
            return None;
        }
        let mut lines: Vec<usize> = self.diagnostics.iter().map(|d| d.line).collect();
        lines.sort_unstable();
        lines.dedup();
        let next = lines
            .iter()
            .copied()
            .find(|&l| l > self.cursor.line)
            .unwrap_or(lines[0]);
        self.cursor = Position { line: next, col: 0 };
        self.selection_anchor = None;
        self.secondary.clear();
        let msgs: Vec<&str> = self
            .diagnostics
            .iter()
            .filter(|d| d.line == next)
            .map(|d| d.message.as_str())
            .collect();
        Some(msgs.join("  |  "))
    }

    pub fn last_edit_instant(&self) -> Option<Instant> {
        self.last_edit_at
    }

    /// El identificador completo sobre el que está parado el cursor
    /// primario, mirando para los dos lados. Es distinto de
    /// `identifier_prefix_before_cursor`, que solo mira hacia atrás: para
    /// renombrar o ir a la definición hace falta la palabra entera, con el
    /// cursor donde sea de ella.
    ///
    /// Si el cursor no está tocando ninguna, devuelve una cadena vacía: eso
    /// es "no hay nada que renombrar acá", no un error.
    pub fn word_under_cursor(&self) -> String {
        let linea = self.cursor.line.min(self.line_count().saturating_sub(1));
        let len = self.line_char_len(linea);
        let chars: Vec<char> = self.rope.line(linea).chars().take(len).collect();
        let es_parte = |c: char| c.is_alphanumeric() || c == '_';
        // Con el cursor justo al final de una palabra (el caso de recién
        // terminar de escribirla) se toma la de la izquierda.
        let mut i = self.cursor.col.min(chars.len());
        if (i >= chars.len() || !es_parte(chars[i])) && i > 0 && es_parte(chars[i - 1]) {
            i -= 1;
        }
        if i >= chars.len() || !es_parte(chars[i]) {
            return String::new();
        }
        let mut ini = i;
        while ini > 0 && es_parte(chars[ini - 1]) {
            ini -= 1;
        }
        let mut fin = i;
        while fin < chars.len() && es_parte(chars[fin]) {
            fin += 1;
        }
        chars[ini..fin].iter().collect()
    }

    /// El identificador que ya está tipeado justo antes del cursor primario
    /// (alfanumérico+`_`, sin cruzar espacios ni puntuación) y dónde
    /// empieza. Lo usa el autocompletado para arrancar el popup ya filtrado
    /// por lo que se escribió antes de pedirlo (`s.pu` + `Ctrl+Espacio`
    /// arranca filtrado por "pu", no vacío) y para saber desde dónde
    /// reemplazar al aceptar una sugerencia.
    /// Palabras que ya están escritas en este buffer y empiezan con `prefix`
    /// (sin distinguir mayúsculas), ordenadas por cercanía a la línea del
    /// cursor y, a igual distancia, alfabéticamente.
    ///
    /// Es el autocompletado que se puede dar en un archivo sin servidor de
    /// lenguaje: no hay más información disponible que el texto mismo. La
    /// cercanía manda porque lo que escribiste hace tres líneas es mucho más
    /// probable que sea lo que querés repetir que algo del otro extremo del
    /// archivo.
    ///
    /// Recorre el buffer entero, pero solo cuando se pide de verdad
    /// (`Ctrl+Espacio`), no en cada tecla; y va línea por línea sobre el rope
    /// en vez de volcarlo a un `String`, igual que la búsqueda literal.
    pub fn words_starting_with(&self, prefix: &str, limit: usize) -> Vec<String> {
        let needle = prefix.to_lowercase();
        let prefix_len = prefix.chars().count();
        // palabra → distancia mínima, en líneas, hasta el cursor.
        let mut best: HashMap<String, usize> = HashMap::new();
        for (i, line) in self.rope.lines().enumerate() {
            let dist = i.abs_diff(self.cursor.line);
            let text: Cow<str> = match line.as_str() {
                Some(s) => Cow::Borrowed(s),
                None => Cow::Owned(line.to_string()),
            };
            for word in text.split_word_bounds() {
                // `split_word_bounds` devuelve también espacios y signos;
                // acá solo interesa lo que puede ser una palabra o un
                // identificador.
                if !word.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                // Una candidata del mismo largo que lo tipeado es lo tipeado
                // mismo (en alguna combinación de mayúsculas): no aporta.
                if word.chars().count() <= prefix_len {
                    continue;
                }
                if !word.to_lowercase().starts_with(&needle) {
                    continue;
                }
                best.entry(word.to_string())
                    .and_modify(|d| *d = (*d).min(dist))
                    .or_insert(dist);
            }
        }
        let mut found: Vec<(usize, String)> = best.into_iter().map(|(w, d)| (d, w)).collect();
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        found.into_iter().take(limit).map(|(_, w)| w).collect()
    }

    pub fn identifier_prefix_before_cursor(&self) -> (Position, String) {
        let line = self.cursor.line;
        let col = self.cursor.col;
        let chars: Vec<char> = self.rope.line(line).chars().take(col).collect();
        let mut start = col;
        while start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
            start -= 1;
        }
        let prefix: String = chars[start..].iter().collect();
        (Position { line, col: start }, prefix)
    }

    pub fn save(&mut self) -> io::Result<()> {
        // Antes de escribir, no después: lo que se guarda y lo que queda en
        // pantalla tienen que ser el mismo texto.
        if self.trim_on_save {
            self.trim_trailing_whitespace();
        }
        let path = self
            .filename
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "sin nombre de archivo"))?;
        let mut file = fs::File::create(&path)?;
        for chunk in self.rope.chunks() {
            file.write_all(chunk.as_bytes())?;
        }
        file.flush()?;
        self.dirty = false;
        // Lo que hay en disco vuelve a ser lo que Flint tiene, así que la
        // fecha de referencia se corre y el respaldo deja de hacer falta.
        self.disk_mtime = mtime_de(&path);
        self.discard_backup();
        self.recien_guardado = true;
        Ok(())
    }

    /// Si el archivo cambió en disco desde que Flint lo leyó o lo guardó por
    /// última vez. Un `git checkout`, un formateador o un `sed -i` corriendo
    /// afuera entran por acá; guardar encima sin avisar borraría ese trabajo.
    pub fn disk_changed(&self) -> bool {
        let Some(path) = &self.filename else {
            return false;
        };
        match (mtime_de(path), self.disk_mtime) {
            (Some(ahora), Some(antes)) => ahora != antes,
            // Existe ahora y no existía cuando se abrió (o al revés): también
            // es un cambio de afuera.
            (a, b) => a.is_some() != b.is_some(),
        }
    }

    /// Dónde vive el respaldo de este archivo: un único directorio, con la
    /// ruta absoluta metida en el nombre (las barras pasadas a `%`) para que
    /// dos archivos que se llaman igual en carpetas distintas no se pisen.
    pub fn backup_path(&self) -> Option<PathBuf> {
        let path = self.filename.as_ref()?;
        let absoluta = fs::canonicalize(path).unwrap_or_else(|_| {
            std::env::current_dir()
                .unwrap_or_default()
                .join(path)
        });
        let plana = absoluta.to_string_lossy().replace(['/', '\\'], "%");
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join(".local/share/flint/backups")
                .join(format!("{plana}.bak")),
        )
    }

    /// Escribe el respaldo si hay algo sin guardar y pasó `cada` desde el
    /// anterior. Devuelve si escribió uno.
    ///
    /// Es una copia entera y no un diario de cambios: para un archivo de
    /// texto es más simple, y lo que se quiere recuperar después de una caída
    /// es el contenido, no la historia.
    pub fn write_backup_if_due(&mut self, cada: Duration) -> bool {
        if !self.dirty {
            return false;
        }
        if self.last_backup.is_some_and(|t| t.elapsed() < cada) {
            return false;
        }
        let Some(destino) = self.backup_path() else {
            return false;
        };
        if let Some(dir) = destino.parent()
            && fs::create_dir_all(dir).is_err()
        {
            return false;
        }
        let Ok(mut file) = fs::File::create(&destino) else {
            return false;
        };
        for chunk in self.rope.chunks() {
            if file.write_all(chunk.as_bytes()).is_err() {
                return false;
            }
        }
        self.last_backup = Some(Instant::now());
        true
    }

    /// Borra el respaldo: ya no hace falta porque lo de disco y lo de
    /// pantalla coinciden.
    pub fn discard_backup(&mut self) {
        if let Some(destino) = self.backup_path() {
            let _ = fs::remove_file(destino);
        }
        self.last_backup = None;
    }

    /// El respaldo que quedó de una sesión anterior, si puede tener trabajo
    /// que el archivo no tiene.
    ///
    /// Con fechas iguales se avisa igual: guardar borra el respaldo, así que
    /// si sigue ahí es porque algo quedó sin guardar, y las fechas empatan
    /// nomás porque el sistema de archivos no distingue tan fino. Avisar de
    /// más molesta; callarse de más pierde trabajo.
    pub fn pending_backup(&self) -> Option<PathBuf> {
        let destino = self.backup_path()?;
        let respaldo = mtime_de(&destino)?;
        match self.filename.as_ref().and_then(|p| mtime_de(p)) {
            Some(archivo) if respaldo < archivo => None,
            _ => Some(destino),
        }
    }

    pub fn line_count(&self) -> usize {
        self.rope.len_lines().max(1)
    }

    pub fn gutter_width(&self) -> u16 {
        let digits = self.line_count().to_string().len().max(2);
        (digits + 1) as u16
    }

    /// Largo en caracteres de una línea, sin contar el terminador (\n o \r\n).
    pub fn line_char_len(&self, line: usize) -> usize {
        let slice = self.rope.line(line);
        let len = slice.len_chars();
        if len == 0 {
            return 0;
        }
        let last = slice.char(len - 1);
        if last == '\n' {
            if len >= 2 && slice.char(len - 2) == '\r' {
                len - 2
            } else {
                len - 1
            }
        } else {
            len
        }
    }

    /// Igual que la función libre `char_display_width`, con el `tab_width`
    /// de este buffer ya puesto.
    pub fn char_display_width(&self, c: char, at: usize) -> usize {
        char_display_width(c, at, self.tab_width)
    }

    /// Columna de pantalla donde empieza a dibujarse el carácter `col` de
    /// `line`. Sin tabuladores es `col` tal cual; con ellos es más grande,
    /// porque cada `\t` ocupa varias columnas aunque sea un solo carácter.
    /// Toda la capa de dibujo (cursor, scroll horizontal, ajuste de línea)
    /// trabaja en estas columnas, no en índices de carácter.
    pub fn display_col(&self, line: usize, col: usize) -> usize {
        if line >= self.line_count() {
            return 0;
        }
        let mut vis = 0;
        for (i, c) in self.rope.line(line).chars().enumerate() {
            if i >= col || c == '\n' || c == '\r' {
                break;
            }
            vis += self.char_display_width(c, vis);
        }
        vis
    }

    /// Inverso de `display_col`: qué carácter de `line` se está dibujando en
    /// la columna de pantalla `target`. Un clic que cae *dentro* de un
    /// tabulador (entre sus columnas) devuelve el tabulador mismo — no hay
    /// posición del buffer a mitad de camino donde poner el cursor.
    pub fn col_from_display(&self, line: usize, target: usize) -> usize {
        if line >= self.line_count() {
            return 0;
        }
        let len = self.line_char_len(line);
        let mut vis = 0;
        for (i, c) in self.rope.line(line).chars().enumerate() {
            if i >= len {
                break;
            }
            let w = self.char_display_width(c, vis);
            if target < vis + w {
                return i;
            }
            vis += w;
        }
        len
    }

    /// Con ajuste de línea activado: en qué columna de pantalla arranca cada
    /// fila visual de `line`. Siempre devuelve al menos una fila (la que
    /// empieza en 0), hasta para una línea vacía.
    ///
    /// El corte no es una grilla fija cada `content_w` columnas: una fila
    /// termina antes si el carácter siguiente no entra entero. Es la
    /// diferencia entre partir un `日` al medio —y no poder dibujar ninguna
    /// de sus dos mitades, que es lo que pasaba antes— y bajarlo completo a
    /// la fila siguiente.
    ///
    /// Es la única fuente de verdad del corte: la dibuja `draw_text_wrapped`,
    /// la cuenta el cálculo de scroll y la deshace `screen_to_pos` para los
    /// clics. Mide con `char_display_width`, igual que todo lo demás.
    pub fn wrap_row_starts(&self, line: usize, content_w: usize) -> Vec<usize> {
        let mut starts = vec![0];
        if content_w == 0 || line >= self.line_count() {
            return starts;
        }
        let len = self.line_char_len(line);
        let mut col = 0;
        let mut row_start = 0;
        for (i, c) in self.rope.line(line).chars().enumerate() {
            if i >= len {
                break;
            }
            let w = self.char_display_width(c, col);
            // `col > row_start` evita quedarse en el molde con una ventana
            // más angosta que un solo carácter: ahí no hay corte posible y
            // el carácter desborda su fila, que es lo menos malo.
            if col + w > row_start + content_w && col > row_start {
                row_start = col;
                starts.push(row_start);
            }
            col += w;
        }
        starts
    }

    /// En qué fila visual de `line` cae la columna de pantalla `vis`.
    pub fn wrap_row_of(&self, starts: &[usize], vis: usize) -> usize {
        starts.partition_point(|&s| s <= vis).saturating_sub(1)
    }

    pub fn pos_to_char_idx(&self, pos: Position) -> usize {
        self.rope.line_to_char(pos.line) + pos.col
    }

    pub fn position_from_char_idx(&self, idx: usize) -> Position {
        let idx = idx.min(self.rope.len_chars());
        let line = self.rope.char_to_line(idx);
        let col = idx - self.rope.line_to_char(line);
        Position { line, col }
    }

    /// El protocolo LSP mide columnas en unidades UTF-16, no en caracteres
    /// Unicode (`Position.col`, lo que usa Flint puertas adentro) — para
    /// cualquier carácter fuera del plano básico (la mayoría de los emoji)
    /// no son el mismo número. Esta conversión es la que hace falta antes de
    /// mandarle una posición al servidor (sincronización, autocompletado).
    pub fn char_col_to_utf16(&self, line: usize, char_col: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        self.rope.line(line).chars().take(char_col).map(char::len_utf16).sum()
    }

    /// La inversa: de una columna UTF-16 (la que manda el servidor, en
    /// diagnósticos por ejemplo) a columna en caracteres. `line` se recorta
    /// a un renglón válido — un diagnóstico puede llegar con un número de
    /// línea fuera de rango si el buffer cambió después de pedirlo.
    pub fn utf16_col_to_char(&self, line: usize, utf16_col: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        let mut chars = 0usize;
        let mut units = 0usize;
        for c in self.rope.line(line).chars() {
            if units >= utf16_col {
                break;
            }
            units += c.len_utf16();
            chars += 1;
        }
        chars
    }

    fn set_cursor_from_char_idx(&mut self, idx: usize) {
        self.cursor = self.position_from_char_idx(idx);
        self.selection_anchor = None;
        self.secondary.clear();
    }

    fn mark_changed(&mut self) {
        self.dirty = true;
        self.highlights_dirty = true;
        self.content_version = self.content_version.wrapping_add(1);
    }

    // ---------- el modelo unificado: primaria + secundarias ----------

    /// Todas las selecciones activas, la primaria primero.
    pub fn selections_snapshot(&self) -> Vec<Selection> {
        let mut v = Vec::with_capacity(1 + self.secondary.len());
        v.push(Selection {
            anchor: self.selection_anchor.unwrap_or(self.cursor),
            cursor: self.cursor,
        });
        v.extend(self.secondary.iter().copied());
        v
    }

    /// Reemplaza el estado de selección completo a partir de una lista en el
    /// mismo orden que devuelve `selections_snapshot` (primaria primero).
    pub fn apply_selections(&mut self, sels: Vec<Selection>) {
        let mut iter = sels.into_iter();
        if let Some(primary) = iter.next() {
            self.cursor = primary.cursor;
            self.selection_anchor = if primary.anchor == primary.cursor {
                None
            } else {
                Some(primary.anchor)
            };
        }
        self.secondary = iter.collect();
    }

    pub fn selection_char_range(&self, sel: Selection) -> (usize, usize) {
        let a = self.pos_to_char_idx(sel.anchor);
        let b = self.pos_to_char_idx(sel.cursor);
        if a <= b {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// Aplica `f` de forma independiente a cada selección (movimiento, o
    /// construir una selección) y guarda el resultado. No toca el buffer.
    fn map_each_selection<F>(&mut self, mut f: F)
    where
        F: FnMut(&Editor, Selection) -> Selection,
    {
        let sels = self.selections_snapshot();
        let mapped: Vec<Selection> = sels.iter().map(|&s| f(self, s)).collect();
        self.apply_selections(mapped);
    }

    fn move_selection<F: FnOnce(Position) -> Position>(
        sel: Selection,
        extend: bool,
        mover: F,
    ) -> Selection {
        let fixed_anchor = if extend { sel.anchor } else { sel.cursor };
        let new_cursor = mover(sel.cursor);
        if extend {
            Selection {
                anchor: fixed_anchor,
                cursor: new_cursor,
            }
        } else {
            Selection::point(new_cursor)
        }
    }

    pub fn selected_text(&self) -> Option<String> {
        let sels = self.selections_snapshot();
        let mut parts = Vec::new();
        for sel in &sels {
            if sel.anchor != sel.cursor {
                let (start, end) = self.selection_char_range(*sel);
                parts.push(self.rope.slice(start..end).to_string());
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    /// El texto que `delete_at_each_selection`/`delete_forward` borraría en
    /// este momento, sin borrar nada todavía — para poder guardarlo en el
    /// registro *antes* de la edición y que "borrar" funcione como "cortar".
    /// Para una selección con rango, es ese rango; para un cursor suelto, el
    /// carácter que tiene delante (o el `\r\n` completo, si el borrado los
    /// fusionaría en un solo paso).
    pub fn text_at_forward_delete_points(&self) -> Option<String> {
        let sels = self.selections_snapshot();
        let mut parts = Vec::new();
        for sel in &sels {
            let (start, end) = self.selection_char_range(*sel);
            if end > start {
                parts.push(self.rope.slice(start..end).to_string());
                continue;
            }
            let len_chars = self.rope.len_chars();
            if start < len_chars {
                let mut stop = start + 1;
                if start + 1 < len_chars
                    && self.rope.char(start) == '\r'
                    && self.rope.char(start + 1) == '\n'
                {
                    stop = start + 2;
                }
                parts.push(self.rope.slice(start..stop).to_string());
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    }

    /// El corazón de toda edición multi-cursor. `plan` decide, para cada
    /// selección (recibe el rope *original*, sin tocar, y el rango de
    /// caracteres que ocupa esa selección), qué rango exacto borrar y qué
    /// texto poner en su lugar.
    ///
    /// El truco: las posiciones resultantes se calculan en una pasada
    /// *aparte*, de izquierda a derecha, acumulando cuánto se corrió el texto
    /// por las ediciones anteriores — así da igual en qué orden se apliquen
    /// de verdad las mutaciones sobre el rope (que sigue siendo de derecha a
    /// izquierda, para que cada `remove`/`insert` use índices todavía
    /// válidos). Calcular la posición final directamente sobre el rope a
    /// medias editado es el error fácil de cometer aquí: una selección ya
    /// procesada queda con una posición que una edición *posterior* (más a
    /// la izquierda) termina corriendo, y el resultado se desalinea.
    fn edit_all_selections<F>(&mut self, kind: EditKind, mut plan: F)
    where
        F: FnMut(&Rope, usize, usize) -> (usize, usize, String),
    {
        self.checkpoint(kind);
        let sels = self.selections_snapshot();

        // (índice original, borrar desde, borrar hasta, texto a insertar) — todo
        // en términos del rope tal como está antes de tocar nada.
        let mut planned: Vec<(usize, usize, usize, String)> = Vec::with_capacity(sels.len());
        for (i, sel) in sels.iter().enumerate() {
            let (s, e) = self.selection_char_range(*sel);
            let (del_start, del_end, text) = plan(&self.rope, s, e);
            planned.push((i, del_start, del_end, text));
        }
        let planned = fundir_ediciones(planned);

        // Para el LSP: una sola edición (el caso normal — tipear, borrar,
        // etc. con un cursor) se puede mandar como delta incremental, con
        // las posiciones calculadas acá mismo, contra el rope todavía
        // intacto. Con multi-cursor hay varias ediciones "simultáneas" que
        // el protocolo de LSP no modela así (asume que cada entrada de
        // `contentChanges` se aplica en secuencia sobre el resultado de la
        // anterior) — en vez de reordenarlas para que calcen, que es fácil
        // de hacer mal y silenciosamente desincronizar el documento del
        // servidor, se pide directamente una resincronización completa.
        if planned.len() == 1 {
            let (_, del_start, del_end, text) = &planned[0];
            let start = self.position_from_char_idx(*del_start);
            let end = self.position_from_char_idx(*del_end);
            // Convertidas acá mismo, contra el rope todavía sin tocar — ver
            // el comentario en `LspEdit`.
            let start_col_utf16 = self.char_col_to_utf16(start.line, start.col);
            let end_col_utf16 = self.char_col_to_utf16(end.line, end.col);
            self.pending_lsp_edits.push(LspEdit {
                start_line: start.line,
                start_col_utf16,
                end_line: end.line,
                end_col_utf16,
                text: text.clone(),
            });
        } else if !planned.is_empty() {
            self.needs_full_lsp_sync = true;
            self.pending_lsp_edits.clear();
        }

        // Posición final de cada cursor, de izquierda a derecha con un delta acumulado.
        let mut by_pos = planned.clone();
        by_pos.sort_by_key(|(_, s, _, _)| *s);
        let mut delta: i64 = 0;
        let mut final_char_idx: Vec<(usize, usize)> = Vec::with_capacity(by_pos.len());
        for (i, del_start, del_end, text) in &by_pos {
            let removed = (*del_end - *del_start) as i64;
            let inserted = text.chars().count() as i64;
            let new_start = (*del_start as i64 + delta) as usize;
            final_char_idx.push((*i, new_start + text.chars().count()));
            delta += inserted - removed;
        }

        // Mutación real del rope, de derecha a izquierda.
        let mut by_pos_desc = planned;
        by_pos_desc.sort_by_key(|(_, s, _, _)| std::cmp::Reverse(*s));
        for (_, del_start, del_end, text) in &by_pos_desc {
            if del_end > del_start {
                self.rope.remove(*del_start..*del_end);
            }
            if !text.is_empty() {
                self.rope.insert(*del_start, text);
            }
        }

        // Quedan solo los cursores de las ediciones que sobrevivieron a la
        // fusión, en su orden original (la primaria, si estaba, sigue
        // primera), y sin repetidos: dos cursores que terminan en el mismo
        // lugar escribirían dos veces cada letra que siga.
        final_char_idx.sort_by_key(|(i, _)| *i);
        let mut vistos = std::collections::HashSet::new();
        let nuevas: Vec<Selection> = final_char_idx
            .into_iter()
            .filter(|(_, idx)| vistos.insert(*idx))
            .map(|(_, idx)| Selection::point(self.position_from_char_idx(idx)))
            .collect();
        self.apply_selections(nuevas);
        self.mark_changed();
    }

    /// Igual que `delete_selection`, pero para cuando el borrado ES la acción
    /// completa del usuario (no un paso interno de `insert_char` y similares).
    pub fn delete_selection_action(&mut self) -> bool {
        self.delete_selection()
    }

    /// Borra el rango de cada selección (primaria o secundaria) que tenga uno
    /// activo; las que son solo un cursor no se tocan. `false` si ninguna
    /// tenía rango — no se registra ningún punto de deshacer en ese caso.
    pub fn delete_selection(&mut self) -> bool {
        let sels = self.selections_snapshot();
        if !sels.iter().any(|s| s.anchor != s.cursor) {
            return false;
        }
        self.edit_all_selections(EditKind::Delete, |_, s, e| (s, e, String::new()));
        true
    }

    /// Busca, desde `cursor.col` en la línea (inclusive), el primer tramo que
    /// cuenta como "palabra" y devuelve su rango — usando los límites de
    /// palabra Unicode reales (UAX #29, vía `unicode-segmentation`), no solo
    /// alfanumérico+`_` carácter por carácter. Eso agrupa bien acentos,
    /// contracciones (`don't` es una sola palabra), CJK y emoji con piel/
    /// modificadores, en vez de cortarlos donde no corresponde.
    ///
    /// Si el cursor cae *dentro* de un tramo de palabra, la selección
    /// arranca ahí (no retrocede al principio del tramo) — mismo
    /// comportamiento de "hacia adelante desde el cursor" que tenía antes.
    fn word_range_from(&self, cursor: Position) -> Option<Selection> {
        let line = cursor.line;
        let len = self.line_char_len(line);
        let line_str: String = self.rope.line(line).chars().take(len).collect();
        let target = cursor.col.min(len);

        let mut char_idx = 0usize;
        for segment in line_str.split_word_bounds() {
            let seg_len = segment.chars().count();
            let seg_end = char_idx + seg_len;
            if seg_end > target {
                let is_word = segment
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_');
                if is_word {
                    let start = char_idx.max(target);
                    return Some(Selection {
                        anchor: Position { line, col: start },
                        cursor: Position { line, col: seg_end },
                    });
                }
            }
            char_idx = seg_end;
        }
        None
    }

    /// Selecciona la palabra bajo el cursor (o la siguiente en la línea) en
    /// cada selección activa. Solo mira la línea de cada una (v1 sencilla,
    /// sin tabla Unicode de límites de palabra).
    pub fn select_word(&mut self) -> bool {
        let mut any = false;
        self.map_each_selection(|ed, sel| match ed.word_range_from(sel.cursor) {
            Some(new_sel) => {
                any = true;
                new_sel
            }
            None => sel,
        });
        any
    }

    /// Selecciona la línea actual completa (con su salto de línea) en cada
    /// selección activa. Si `extend_streak` es true (la tecla anterior
    /// también fue "seleccionar línea"), extiende una línea más hacia abajo
    /// en vez de empezar una nueva — igual que la `x` repetida de Kakoune.
    pub fn select_line(&mut self, extend_streak: bool) {
        self.map_each_selection(|ed, sel| {
            let anchor = if extend_streak && sel.anchor != sel.cursor {
                sel.anchor
            } else {
                Position {
                    line: sel.cursor.line,
                    col: 0,
                }
            };
            let new_cursor = if sel.cursor.line + 1 < ed.line_count() {
                Position {
                    line: sel.cursor.line + 1,
                    col: 0,
                }
            } else {
                Position {
                    line: sel.cursor.line,
                    col: ed.line_char_len(sel.cursor.line),
                }
            };
            Selection {
                anchor,
                cursor: new_cursor,
            }
        });
    }

    /// Agrega una selección nueva sobre la siguiente aparición del texto de
    /// la selección de referencia (la última que tenga un rango activo),
    /// saltando las que ya están elegidas y dando la vuelta si hace falta.
    /// Así se arma el multi-cursor: seleccionar algo, repetir esta acción.
    pub fn select_next_occurrence(&mut self) -> bool {
        let sels = self.selections_snapshot();
        let Some(reference) = sels.iter().rev().find(|s| s.anchor != s.cursor).copied() else {
            return false;
        };
        let (ref_start, ref_end) = self.selection_char_range(reference);
        if ref_end <= ref_start {
            return false;
        }
        let text = self.rope.to_string();
        let pattern: String = text.chars().skip(ref_start).take(ref_end - ref_start).collect();
        if pattern.is_empty() {
            return false;
        }

        let mut matches: Vec<(usize, usize)> = Vec::new();
        let mut byte_pos = 0;
        while let Some(rel) = text[byte_pos..].find(&pattern) {
            let byte_idx = byte_pos + rel;
            let char_start = text[..byte_idx].chars().count();
            let char_end = char_start + pattern.chars().count();
            matches.push((char_start, char_end));
            byte_pos = byte_idx + pattern.len().max(1);
            if matches.len() >= MAX_OCCURRENCES {
                break;
            }
        }
        if matches.is_empty() {
            return false;
        }

        let existing: std::collections::HashSet<(usize, usize)> =
            sels.iter().map(|s| self.selection_char_range(*s)).collect();

        let candidate = matches
            .iter()
            .filter(|&&(s, _)| s >= ref_end)
            .chain(matches.iter())
            .find(|m| !existing.contains(m));

        let Some(&(m_start, m_end)) = candidate else {
            return false;
        };

        self.secondary.push(Selection {
            anchor: self.position_from_char_idx(m_start),
            cursor: self.position_from_char_idx(m_end),
        });
        true
    }

    /// Registra el estado actual como punto al que volver, a menos que la edición
    /// anterior sea del mismo tipo y reciente (así "escribir" o "borrar" seguido
    /// se deshace de una sola vez, no letra por letra).
    fn checkpoint(&mut self, kind: EditKind) {
        let now = Instant::now();
        let same_group = self.last_edit_kind == Some(kind)
            && self
                .last_edit_at
                .is_some_and(|t| now.duration_since(t) < GROUP_TIMEOUT);
        if !same_group {
            self.undo_stack.push(UndoEntry {
                rope: self.rope.clone(),
                selections: self.selections_snapshot(),
            });
            self.redo_stack.clear();
            if self.undo_stack.len() > MAX_UNDO {
                self.undo_stack.remove(0);
            }
        }
        self.last_edit_kind = Some(kind);
        self.last_edit_at = Some(now);
    }

    pub fn clamp_position(&self, pos: Position) -> Position {
        let max_line = self.line_count().saturating_sub(1);
        let line = pos.line.min(max_line);
        let col = pos.col.min(self.line_char_len(line));
        Position { line, col }
    }

    fn clamp_all_selections(&mut self) {
        let sels = self.selections_snapshot();
        let clamped: Vec<Selection> = sels
            .into_iter()
            .map(|s| Selection {
                anchor: self.clamp_position(s.anchor),
                cursor: self.clamp_position(s.cursor),
            })
            .collect();
        self.apply_selections(clamped);
    }

    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack.push(UndoEntry {
            rope: self.rope.clone(),
            selections: self.selections_snapshot(),
        });
        self.rope = entry.rope;
        self.apply_selections(entry.selections);
        self.clamp_all_selections();
        self.mark_changed();
        self.last_edit_kind = None;
        self.last_edit_at = None;
        self.needs_full_lsp_sync = true;
        self.pending_lsp_edits.clear();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(entry) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack.push(UndoEntry {
            rope: self.rope.clone(),
            selections: self.selections_snapshot(),
        });
        self.rope = entry.rope;
        self.apply_selections(entry.selections);
        self.clamp_all_selections();
        self.mark_changed();
        self.last_edit_kind = None;
        self.last_edit_at = None;
        self.needs_full_lsp_sync = true;
        self.pending_lsp_edits.clear();
        true
    }

    // ---------- edición: siempre de derecha a izquierda sobre todas las selecciones ----------

    pub fn insert_char(&mut self, c: char) {
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf).to_string();
        self.edit_all_selections(EditKind::Insert, move |_, start, end| (start, end, s.clone()));
    }

    /// Auto-indentación: la línea nueva arranca con el mismo espacio en
    /// blanco inicial (espacios o tabs) que la línea donde estaba el
    /// cursor — no intenta ser "inteligente" con llaves/paréntesis (eso
    /// dependería del lenguaje), solo mantiene el nivel actual, que es lo
    /// que espera casi cualquier editor por defecto. Cada selección calcula
    /// la indentación de *su propia* línea, contra el rope todavía intacto.
    pub fn insert_newline(&mut self) {
        // Con un solo cursor y sin selección se puede mirar el contexto y
        // decidir bien. Con varios, cada uno tiene el suyo: ahí se mantiene
        // el comportamiento de siempre (copiar la sangría de cada línea),
        // que es correcto para todos y no inventa nada para nadie.
        if self.secondary.is_empty() && self.selection_anchor.is_none() {
            self.insert_newline_smart();
            return;
        }
        let le = self.line_ending.as_str();
        self.edit_all_selections(EditKind::Other, move |rope, start, end| {
            let line = rope.char_to_line(start.min(rope.len_chars()));
            let indent = Self::leading_whitespace(rope, line);
            (start, end, format!("{le}{indent}"))
        });
    }

    /// La línea nueva hereda la sangría de la actual y suma un nivel si el
    /// cursor quedó adentro de algo que se abrió y no se cerró. Si además el
    /// cierre estaba pegado al cursor, baja a su propia línea y el cursor
    /// queda en el medio — que es lo que uno quiere después de escribir `{`.
    fn insert_newline_smart(&mut self) {
        let le = self.line_ending.as_str().to_string();
        let base = Self::leading_whitespace(&self.rope, self.cursor.line);

        if !self.opens_block_before(self.cursor) {
            let texto = format!("{le}{base}");
            self.edit_all_selections(EditKind::Other, move |_, s, e| (s, e, texto.clone()));
            return;
        }

        let adentro = format!("{base}{}", self.indent_unit());
        let cierre_pegado = self
            .char_after_cursor()
            .is_some_and(|c| Self::opening_for(c).is_some());

        if !cierre_pegado {
            let texto = format!("{le}{adentro}");
            self.edit_all_selections(EditKind::Other, move |_, s, e| (s, e, texto.clone()));
            return;
        }

        let ancho_adentro = adentro.chars().count();
        let texto = format!("{le}{adentro}{le}{base}");
        self.edit_all_selections(EditKind::Other, move |_, s, e| (s, e, texto.clone()));
        // La inserción dejó el cursor al final de todo, o sea en la línea del
        // cierre; el lugar donde uno va a escribir es la del medio.
        self.cursor = Position {
            line: self.cursor.line.saturating_sub(1),
            col: ancho_adentro,
        };
        self.selection_anchor = None;
    }

    /// Si lo que hay entre el principio de la línea y `pos` deja un bloque
    /// abierto: un delimitador sin cerrar, o (en Python) un `:` al final.
    ///
    /// Los delimitadores dentro de una cadena o un comentario no cuentan,
    /// igual que en `matching_bracket`: eso lo sabe el árbol de tree-sitter,
    /// así que un `"{"` adentro de un texto no sangra la línea siguiente.
    fn opens_block_before(&self, pos: Position) -> bool {
        let linea = self.rope.line(pos.line);
        let hasta = pos.col.min(self.line_char_len(pos.line));
        let mut nivel = 0i32;
        for col in 0..hasta {
            let c = linea.char(col);
            if Self::par_de(c).is_none() {
                continue;
            }
            if self.is_in_string_or_comment(Position { line: pos.line, col }) {
                continue;
            }
            if matches!(c, '(' | '[' | '{') {
                nivel += 1;
            } else {
                nivel -= 1;
            }
        }
        if nivel > 0 {
            return true;
        }
        if self.indent_after_colon {
            let texto: String = (0..hasta).map(|col| linea.char(col)).collect();
            return texto.trim_end().ends_with(':');
        }
        false
    }

    /// ¿Alguna selección abarca más de una línea? Es lo que decide si Tab
    /// indenta el bloque o inserta un tabulador suelto: reemplazar varias
    /// líneas seleccionadas por un `\t` nunca es lo que alguien quiso.
    pub fn selection_spans_lines(&self) -> bool {
        self.selections_snapshot()
            .iter()
            .any(|s| s.anchor.line != s.cursor.line)
    }

    /// Las líneas que tocan las selecciones actuales, sin repetir y en orden.
    /// Una selección que termina justo en la columna 0 de una línea (lo que
    /// pasa al arrastrar hasta el renglón siguiente) no cuenta esa última
    /// línea: no hay nada suyo seleccionado ahí, e indentarla sorprendería.
    fn lines_touched(&self) -> Vec<usize> {
        let last_line = self.line_count().saturating_sub(1);
        let mut lines: Vec<usize> = Vec::new();
        for sel in self.selections_snapshot() {
            let (start, end) = if (sel.anchor.line, sel.anchor.col) <= (sel.cursor.line, sel.cursor.col) {
                (sel.anchor, sel.cursor)
            } else {
                (sel.cursor, sel.anchor)
            };
            let end_line = if end.line > start.line && end.col == 0 {
                end.line - 1
            } else {
                end.line
            };
            lines.extend(start.line..=end_line.min(last_line));
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    /// Un nivel de indentación tal como se escribe en el archivo: un
    /// tabulador, o `tab_width` espacios si el archivo (o la configuración
    /// para él) los prefiere.
    pub fn indent_unit(&self) -> String {
        if self.indent_with_spaces {
            " ".repeat(self.tab_width.max(1))
        } else {
            "\t".to_string()
        }
    }

    /// Inserta un nivel de indentación donde está cada cursor — lo que hace
    /// Tab cuando no hay un bloque de varias líneas seleccionado.
    pub fn insert_indent(&mut self) {
        let unidad = self.indent_unit();
        self.edit_all_selections(EditKind::Insert, move |_, start, end| {
            (start, end, unidad.clone())
        });
    }

    /// Suma un nivel de indentación al principio de cada línea que toquen las
    /// selecciones, y las conserva — así se puede apretar Tab varias veces
    /// seguidas sobre el mismo bloque, en vez de perder la selección en la
    /// primera.
    pub fn indent_lines(&mut self) {
        let lines = self.lines_touched();
        if lines.is_empty() {
            return;
        }
        self.checkpoint(EditKind::Other);
        let mut sels = self.selections_snapshot();
        let unidad = self.indent_unit();
        let ancho = unidad.chars().count();
        // De abajo hacia arriba: insertar en una línea no corre los índices
        // de carácter de las que están más arriba.
        for &line in lines.iter().rev() {
            let at = self.rope.line_to_char(line);
            self.rope.insert(at, &unidad);
        }
        for sel in &mut sels {
            for p in [&mut sel.anchor, &mut sel.cursor] {
                // La columna 0 se queda en 0: es donde suele estar el extremo
                // de una selección de líneas enteras, y correrla dejaría el
                // bloque seleccionado a partir del segundo carácter.
                if p.col > 0 && lines.binary_search(&p.line).is_ok() {
                    p.col += ancho;
                }
            }
        }
        self.apply_selections(sels);
        self.after_multiline_edit();
    }

    /// Cierre automático de pares. El de comillas está en la misma tabla
    /// porque se escribe igual, aunque abre y cierra con el mismo carácter.
    pub fn closing_for(c: char) -> Option<char> {
        match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            _ => None,
        }
    }

    fn opening_for(c: char) -> Option<char> {
        match c {
            ')' => Some('('),
            ']' => Some('['),
            '}' => Some('{'),
            _ => None,
        }
    }

    /// El carácter que está justo después del cursor, si hay alguno en esa
    /// misma línea.
    fn char_after_cursor(&self) -> Option<char> {
        let linea = self.rope.line(self.cursor.line);
        (self.cursor.col < self.line_char_len(self.cursor.line)).then(|| linea.char(self.cursor.col))
    }

    fn char_before_cursor(&self) -> Option<char> {
        (self.cursor.col > 0).then(|| self.rope.line(self.cursor.line).char(self.cursor.col - 1))
    }

    /// Escribe `c` con el cierre automático puesto, si corresponde. Devuelve
    /// qué pasó, para poder contarlo en la barra de estado si hiciera falta.
    ///
    /// Con varios cursores se escribe el carácter y nada más: cada cursor
    /// tiene su propio contexto (uno puede estar antes de un `)` y otro no)
    /// y adivinar uno solo para todos daría un texto que nadie pidió.
    pub fn insert_char_pairing(&mut self, c: char) {
        self.insertar_con_par(c);
        if matches!(c, ')' | ']' | '}' | ':') {
            self.desindentar_al_cerrar(c);
        }
    }

    fn insertar_con_par(&mut self, c: char) {
        if !self.auto_close || !self.secondary.is_empty() || self.selection_anchor.is_some() {
            self.insert_char(c);
            return;
        }
        let siguiente = self.char_after_cursor();

        // Escribir el cierre que ya está puesto no duplica: se pasa por
        // encima, que es lo que uno quiere después de tipear adentro del par.
        if Self::opening_for(c).is_some() && siguiente == Some(c) {
            self.move_right(false);
            return;
        }
        let Some(cierre) = Self::closing_for(c) else {
            self.insert_char(c);
            return;
        };
        // Una comilla en medio de una palabra es un apóstrofo (o una vida de
        // Rust), no el principio de una cadena.
        if cierre == c
            && self
                .char_before_cursor()
                .is_some_and(|p| p.is_alphanumeric() || p == '_')
        {
            self.insert_char(c);
            return;
        }
        // Solo se cierra cuando lo que sigue es el borde de la línea, un
        // espacio o el cierre de otro par: pegado a texto, el cierre nuevo
        // quedaría en el medio de algo que ya estaba escrito.
        let hay_lugar = match siguiente {
            None => true,
            Some(s) => s.is_whitespace() || Self::opening_for(s).is_some(),
        };
        if !hay_lugar {
            self.insert_char(c);
            return;
        }
        let mut texto = String::with_capacity(2);
        texto.push(c);
        texto.push(cierre);
        self.edit_all_selections(EditKind::Insert, move |_, start, end| {
            (start, end, texto.clone())
        });
        self.move_left(false);
    }

    /// Si el cursor está justo entre un par vacío (`()` con el cursor en el
    /// medio), borra los dos. Devuelve si lo hizo.
    pub fn backspace_pair(&mut self) -> bool {
        if !self.auto_close || !self.secondary.is_empty() || self.selection_anchor.is_some() {
            return false;
        }
        let (Some(antes), Some(despues)) = (self.char_before_cursor(), self.char_after_cursor())
        else {
            return false;
        };
        if Self::closing_for(antes) != Some(despues) {
            return false;
        }
        self.delete_forward();
        self.backspace();
        true
    }

    /// Después de escribir `c`: si la línea quedó más sangrada que el bloque
    /// al que pertenece, se le quita lo que sobra. Pasa con un cierre que es
    /// lo primero de su línea (toma la sangría de la línea de su apertura) y,
    /// en Python, con el `:` de `else`, `elif`, `except` y `finally` (toma la
    /// de su `if` o `try`). Solo quita sangría, nunca agrega: si el usuario
    /// la dejó más a la izquierda, sabrá por qué. Es un paso de deshacer
    /// aparte, así que Ctrl+Z devuelve la sangría y deja lo escrito.
    fn desindentar_al_cerrar(&mut self, c: char) {
        if !self.secondary.is_empty() || self.selection_anchor.is_some() || self.cursor.col == 0 {
            return;
        }
        let linea = self.cursor.line;
        let antes: String = self.rope.line(linea).chars().take(self.cursor.col).collect();
        let objetivo = if c == ':' {
            if !self.indent_after_colon {
                return;
            }
            match self.sangria_de_bloque_python(linea, antes.trim()) {
                Some(s) => s,
                None => return,
            }
        } else {
            if antes.trim_start() != c.to_string() {
                return;
            }
            let cierre = Position { line: linea, col: self.cursor.col - 1 };
            match self.matching_bracket(cierre) {
                Some(par) if par.line < linea => Self::leading_whitespace(&self.rope, par.line),
                _ => return,
            }
        };
        let actual = Self::leading_whitespace(&self.rope, linea);
        let (n_actual, n_objetivo) = (actual.chars().count(), objetivo.chars().count());
        if n_objetivo >= n_actual {
            return;
        }
        let col = self.cursor.col - n_actual + n_objetivo;
        self.replace_ranges(&[(
            Position { line: linea, col: 0 },
            Position { line: linea, col: n_actual },
            objetivo,
        )]);
        self.cursor = Position { line: linea, col };
    }

    /// La sangría del bloque al que pertenece una línea de Python que empieza
    /// con `else`, `elif`, `except` o `finally` y termina en `:`: la de la
    /// primera línea de arriba con menos sangría, si esa línea abre un bloque
    /// que admite esta continuación. Si no la admite, `None`: mejor no tocar
    /// nada que adivinar.
    fn sangria_de_bloque_python(&self, linea: usize, texto: &str) -> Option<String> {
        if !texto.ends_with(':') {
            return None;
        }
        let palabra = |t: &str| -> String { t.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect() };
        let admitidos: &[&str] = match palabra(texto).as_str() {
            "else" => &["if", "elif", "for", "while", "try", "except"],
            "elif" => &["if", "elif"],
            "except" => &["try", "except"],
            "finally" => &["try", "except", "else"],
            _ => return None,
        };
        let propia = Self::leading_whitespace(&self.rope, linea).chars().count();
        for arriba in (0..linea).rev() {
            let sangria = Self::leading_whitespace(&self.rope, arriba);
            let resto: String = self.rope.line(arriba).chars().skip(sangria.chars().count()).collect();
            let resto = resto.trim_end();
            if resto.is_empty() || resto.starts_with('#') || sangria.chars().count() >= propia {
                continue;
            }
            return admitidos.contains(&palabra(resto).as_str()).then_some(sangria);
        }
        None
    }

    /// La posición del paréntesis, corchete o llave que hace pareja con el
    /// que está en `pos` (o justo antes, que es donde queda el cursor después
    /// de escribirlo). `None` si ahí no hay ninguno, o si no tiene pareja.
    ///
    /// Los que están dentro de una cadena o de un comentario no cuentan: eso
    /// lo sabe el resaltado, que viene del árbol de tree-sitter, así que un
    /// `"("` suelto adentro de un texto no descuadra la cuenta como sí pasa
    /// en los editores que solo cuentan caracteres.
    pub fn matching_bracket(&self, pos: Position) -> Option<Position> {
        self.bracket_pair_at(pos).map(|(_, destino)| destino)
    }

    /// Igual que `matching_bracket`, pero devuelve las dos puntas: el
    /// delimitador que se tomó como punto de partida y su pareja. Es lo que
    /// necesita el dibujado para resaltar los dos a la vez.
    pub fn bracket_pair_at(&self, pos: Position) -> Option<(Position, Position)> {
        let en = |p: Position| -> Option<char> {
            (p.col < self.line_char_len(p.line)).then(|| self.rope.line(p.line).char(p.col))
        };
        // Se prueba primero el carácter bajo el cursor y después el de atrás:
        // al terminar de escribir `foo()` el cursor queda después del `)`, y
        // ahí es donde uno espera que el salto funcione.
        let (inicio, c) = match en(pos).and_then(|c| Self::par_de(c).map(|_| (pos, c))) {
            Some(x) => x,
            None => {
                let antes = Position { line: pos.line, col: pos.col.checked_sub(1)? };
                let c = en(antes)?;
                Self::par_de(c)?;
                (antes, c)
            }
        };
        if self.is_in_string_or_comment(inicio) {
            return None;
        }
        let (objetivo, hacia_adelante) = Self::par_de(c)?;
        let destino = self.scan_for_bracket(inicio, c, objetivo, hacia_adelante)?;
        Some((inicio, destino))
    }

    /// El par de un delimitador y hacia dónde hay que buscarlo.
    fn par_de(c: char) -> Option<(char, bool)> {
        match c {
            '(' => Some((')', true)),
            '[' => Some((']', true)),
            '{' => Some(('}', true)),
            ')' => Some(('(', false)),
            ']' => Some(('[', false)),
            '}' => Some(('{', false)),
            _ => None,
        }
    }

    /// Si esa posición cae adentro de una cadena o un comentario según el
    /// último resaltado calculado. Sin resaltado (lenguaje desconocido, o
    /// todavía sin calcular) contesta que no: contar de más es mejor que
    /// ignorar delimitadores de verdad.
    fn is_in_string_or_comment(&self, pos: Position) -> bool {
        let Some(rangos) = self.highlights_by_line.get(pos.line) else {
            return false;
        };
        rangos.iter().any(|&(desde, hasta, kind)| {
            matches!(kind, HighlightKind::String | HighlightKind::Comment)
                && (desde..hasta).contains(&pos.col)
        })
    }

    /// Recorre el documento contando anidamiento hasta encontrar la pareja.
    /// El tope de caracteres evita que un archivo enorme con un delimitador
    /// suelto convierta cada pulsación en un recorrido completo.
    fn scan_for_bracket(
        &self,
        desde: Position,
        abre: char,
        cierra: char,
        hacia_adelante: bool,
    ) -> Option<Position> {
        const MAX_PASOS: usize = 500_000;
        let mut nivel = 0i32;
        let mut pos = desde;
        for _ in 0..MAX_PASOS {
            pos = if hacia_adelante {
                self.next_position(pos)?
            } else {
                self.prev_position(pos)?
            };
            let linea = self.rope.line(pos.line);
            if pos.col >= self.line_char_len(pos.line) {
                continue;
            }
            let c = linea.char(pos.col);
            if (c == abre || c == cierra) && !self.is_in_string_or_comment(pos) {
                if c == abre {
                    nivel += 1;
                } else if nivel == 0 {
                    return Some(pos);
                } else {
                    nivel -= 1;
                }
            }
        }
        None
    }

    fn next_position(&self, pos: Position) -> Option<Position> {
        if pos.col < self.line_char_len(pos.line) {
            return Some(Position { line: pos.line, col: pos.col + 1 });
        }
        (pos.line + 1 < self.line_count()).then_some(Position { line: pos.line + 1, col: 0 })
    }

    fn prev_position(&self, pos: Position) -> Option<Position> {
        if pos.col > 0 {
            return Some(Position { line: pos.line, col: pos.col - 1 });
        }
        pos.line.checked_sub(1).map(|line| Position {
            line,
            col: self.line_char_len(line),
        })
    }

    /// Lleva el cursor al delimitador que hace pareja con el de al lado.
    pub fn jump_to_matching_bracket(&mut self) -> bool {
        let Some(destino) = self.matching_bracket(self.cursor) else {
            return false;
        };
        self.cursor = destino;
        self.selection_anchor = None;
        self.secondary.clear();
        true
    }

    /// Comenta o descomenta las líneas que toquen las selecciones, con el
    /// token de una línea del lenguaje (`//`, `#`, …). Descomenta solo si
    /// **todas** las líneas con texto ya estaban comentadas; si hay una
    /// mezcla, comenta todo — que es lo que uno espera al apretar una vez
    /// sobre un bloque a medio comentar.
    ///
    /// El token entra en la sangría *mínima* del bloque y no en la de cada
    /// línea, así que la escalera de indentación de adentro se conserva.
    /// Devuelve si tocó algo: las líneas en blanco no cuentan, y un bloque
    /// entero en blanco no gasta un paso de deshacer.
    pub fn toggle_line_comment(&mut self, token: &str) -> bool {
        let con_texto: Vec<usize> = self
            .lines_touched()
            .into_iter()
            .filter(|&l| self.rope.line(l).chars().any(|c| !c.is_whitespace()))
            .collect();
        if con_texto.is_empty() {
            return false;
        }
        let n_token = token.chars().count();
        let comentadas = con_texto.iter().all(|&l| {
            let sangria = Self::leading_whitespace(&self.rope, l).chars().count();
            self.rope
                .line(l)
                .chars()
                .skip(sangria)
                .take(n_token)
                .eq(token.chars())
        });

        self.checkpoint(EditKind::Other);
        let mut sels = self.selections_snapshot();
        // (línea, columna donde se editó, cuánto se corrió con signo).
        let mut cambios: Vec<(usize, usize, isize)> = Vec::new();

        if comentadas {
            // De abajo hacia arriba, como en indent_lines: editar una línea
            // no corre los índices de carácter de las de más arriba.
            for &l in con_texto.iter().rev() {
                let sangria = Self::leading_whitespace(&self.rope, l).chars().count();
                // Se saca también el espacio que se puso al comentar, si
                // sigue ahí — si no, descomentar dejaría un margen que crece
                // con cada vuelta.
                let con_espacio = self.rope.line(l).chars().nth(sangria + n_token) == Some(' ');
                let n = n_token + usize::from(con_espacio);
                let inicio = self.rope.line_to_char(l) + sangria;
                self.rope.remove(inicio..inicio + n);
                cambios.push((l, sangria, -(n as isize)));
            }
        } else {
            let col = con_texto
                .iter()
                .map(|&l| Self::leading_whitespace(&self.rope, l).chars().count())
                .min()
                .unwrap_or(0);
            let texto = format!("{token} ");
            let n = texto.chars().count() as isize;
            for &l in con_texto.iter().rev() {
                let at = self.rope.line_to_char(l) + col;
                self.rope.insert(at, &texto);
                cambios.push((l, col, n));
            }
        }

        for sel in &mut sels {
            for p in [&mut sel.anchor, &mut sel.cursor] {
                if let Some(&(_, col, delta)) = cambios.iter().find(|&&(l, _, _)| l == p.line)
                    && p.col >= col
                {
                    p.col = p.col.saturating_add_signed(delta);
                }
            }
        }
        self.apply_selections(sels);
        self.clamp_all_selections();
        self.after_multiline_edit();
        true
    }

    /// Lleva el cursor al principio de `line` (contada desde 0), sin
    /// selección ni cursores extra. El desplazamiento de pantalla lo ajusta
    /// el dibujado, que ya sigue al cursor esté donde esté.
    pub fn goto_line(&mut self, line: usize) {
        let line = line.min(self.line_count().saturating_sub(1));
        self.cursor = Position { line, col: 0 };
        self.selection_anchor = None;
        self.secondary.clear();
    }

    /// Reemplaza varios rangos a la vez, como un solo paso de deshacer.
    ///
    /// Es lo que necesita un renombre del servidor de lenguaje, que no llega
    /// como una edición del usuario sino como una lista de rangos contra el
    /// documento tal como está: un `Ctrl+Z` tiene que deshacer el renombre
    /// entero de este archivo, no ocurrencia por ocurrencia.
    ///
    /// Se aplican de atrás para adelante porque todos los rangos están
    /// expresados contra el documento original: hacerlo de adelante para
    /// atrás correría los de más abajo y cada uno caería más desalineado que
    /// el anterior. El protocolo garantiza que no se superponen.
    ///
    /// Devuelve cuántos se aplicaron. Cero significa que no se tocó nada y
    /// tampoco se gastó un paso de deshacer.
    pub fn replace_ranges(&mut self, cambios: &[(Position, Position, String)]) -> usize {
        if cambios.is_empty() {
            return 0;
        }
        let mut ordenados: Vec<(usize, usize, &str)> = cambios
            .iter()
            .map(|(desde, hasta, texto)| {
                let ini = self.pos_to_char_idx(self.clamp_position(*desde));
                let fin = self.pos_to_char_idx(self.clamp_position(*hasta));
                (ini.min(fin), ini.max(fin), texto.as_str())
            })
            .collect();
        ordenados.sort_by_key(|(ini, _, _)| std::cmp::Reverse(*ini));

        // Un punto de deshacer propio, sin agrupar con lo que el usuario
        // venía escribiendo: `Other` más el corte del agrupado por tiempo.
        self.last_edit_kind = None;
        self.checkpoint(EditKind::Other);

        for (ini, fin, texto) in &ordenados {
            if fin > ini {
                self.rope.remove(*ini..*fin);
            }
            if !texto.is_empty() {
                self.rope.insert(*ini, texto);
            }
        }
        // El cursor puede haber quedado más allá del final de su línea si lo
        // que se reemplazó era más largo que lo que entró.
        self.clamp_all_selections();
        // Estas ediciones no son una secuencia de deltas del usuario: la
        // próxima sincronización manda el documento completo.
        self.needs_full_lsp_sync = true;
        self.pending_lsp_edits.clear();
        self.mark_changed();
        ordenados.len()
    }

    /// Saca los espacios y tabuladores del final de cada línea. Devuelve si
    /// tocó algo, para que quien llama sepa si hubo edición de verdad (y no
    /// gaste un paso de deshacer ni marque el buffer sucio si no la hubo).
    /// Se usa al guardar, donde el cursor puede quedar más allá del final de
    /// su línea: por eso al terminar se reencuadran todas las selecciones.
    pub fn trim_trailing_whitespace(&mut self) -> bool {
        let recortes: Vec<(usize, usize)> = (0..self.line_count())
            .filter_map(|line| {
                let len = self.line_char_len(line);
                let slice = self.rope.line(line);
                let mut n = 0;
                while n < len {
                    let c = slice.char(len - 1 - n);
                    if c == ' ' || c == '\t' {
                        n += 1;
                    } else {
                        break;
                    }
                }
                (n > 0).then_some((line, n))
            })
            .collect();
        if recortes.is_empty() {
            return false;
        }
        self.checkpoint(EditKind::Other);
        for &(line, n) in recortes.iter().rev() {
            let fin = self.rope.line_to_char(line) + self.line_char_len(line);
            self.rope.remove(fin - n..fin);
        }
        self.clamp_all_selections();
        self.after_multiline_edit();
        true
    }

    /// Lo contrario: saca un nivel de indentación de cada línea tocada — un
    /// `\t`, o hasta `tab_width` espacios si esa línea usa espacios. Las
    /// líneas que ya empiezan pegadas al margen se dejan como están.
    pub fn unindent_lines(&mut self) {
        // Cuánto sacarle a cada línea, calculado contra el rope intacto. Si
        // no hay nada que sacar en ninguna, se vuelve sin tocar el buffer:
        // ni marcarlo como sucio ni gastar un paso de deshacer.
        let removals: Vec<(usize, usize)> = self
            .lines_touched()
            .into_iter()
            .filter_map(|line| {
                let n = self.indent_chars_to_remove(line);
                (n > 0).then_some((line, n))
            })
            .collect();
        if removals.is_empty() {
            return;
        }
        self.checkpoint(EditKind::Other);
        let mut sels = self.selections_snapshot();
        for &(line, n) in removals.iter().rev() {
            let at = self.rope.line_to_char(line);
            self.rope.remove(at..at + n);
        }
        for sel in &mut sels {
            for p in [&mut sel.anchor, &mut sel.cursor] {
                if let Ok(i) = removals.binary_search_by_key(&p.line, |&(l, _)| l) {
                    p.col = p.col.saturating_sub(removals[i].1);
                }
            }
        }
        self.apply_selections(sels);
        self.after_multiline_edit();
    }

    /// Cuántos caracteres hay que sacarle al principio de `line` para quitarle
    /// un nivel de indentación: 1 si arranca con tabulador, o los espacios que
    /// tenga hasta un máximo de `tab_width`.
    fn indent_chars_to_remove(&self, line: usize) -> usize {
        let mut n = 0;
        for (i, c) in self.rope.line(line).chars().enumerate() {
            if i == 0 && c == '\t' {
                return 1;
            }
            if c == ' ' && i < self.tab_width {
                n = i + 1;
            } else {
                break;
            }
        }
        n
    }

    /// Cierre común de indentar/des-indentar: son varias ediciones a la vez,
    /// que el protocolo de LSP no modela como deltas simultáneos (ver el
    /// comentario en `edit_all_selections`), así que se pide resincronización
    /// completa en vez de arriesgar un documento desalineado del lado del
    /// servidor.
    fn after_multiline_edit(&mut self) {
        self.needs_full_lsp_sync = true;
        self.pending_lsp_edits.clear();
        self.mark_changed();
    }

    /// El espacio en blanco (espacios/tabs) al principio de `line`.
    fn leading_whitespace(rope: &Rope, line: usize) -> String {
        let mut s = String::new();
        for c in rope.line(line).chars() {
            if c == ' ' || c == '\t' {
                s.push(c);
            } else {
                break;
            }
        }
        s
    }

    pub fn backspace(&mut self) {
        self.edit_all_selections(EditKind::Delete, |rope, start, end| {
            if end > start {
                (start, end, String::new())
            } else if start > 0 {
                let mut del_start = start - 1;
                if start >= 2 {
                    let prev = rope.char(start - 1);
                    let prev2 = rope.char(start - 2);
                    if prev == '\n' && prev2 == '\r' {
                        del_start = start - 2;
                    }
                }
                (del_start, start, String::new())
            } else {
                (start, start, String::new())
            }
        });
    }

    pub fn delete_forward(&mut self) {
        self.edit_all_selections(EditKind::Delete, |rope, start, end| {
            if end > start {
                (start, end, String::new())
            } else {
                let len_chars = rope.len_chars();
                if start < len_chars {
                    let mut del_end = start + 1;
                    if start + 1 < len_chars {
                        let c0 = rope.char(start);
                        let c1 = rope.char(start + 1);
                        if c0 == '\r' && c1 == '\n' {
                            del_end = start + 2;
                        }
                    }
                    (start, del_end, String::new())
                } else {
                    (start, start, String::new())
                }
            }
        });
    }

    /// La acción "borrar" de la capa modal: en cada selección, borra su rango
    /// si tiene uno; si no, borra el carácter siguiente al cursor. A
    /// diferencia de `delete_selection_action`, nunca es un no-op.
    pub fn delete_at_each_selection(&mut self) {
        self.delete_forward();
    }

    pub fn move_left(&mut self, extend: bool) {
        self.map_each_selection(|ed, sel| {
            Self::move_selection(sel, extend, |cur| {
                if cur.col > 0 {
                    Position {
                        line: cur.line,
                        col: cur.col - 1,
                    }
                } else if cur.line > 0 {
                    let line = cur.line - 1;
                    Position {
                        line,
                        col: ed.line_char_len(line),
                    }
                } else {
                    cur
                }
            })
        });
    }

    pub fn move_right(&mut self, extend: bool) {
        self.map_each_selection(|ed, sel| {
            Self::move_selection(sel, extend, |cur| {
                let len = ed.line_char_len(cur.line);
                if cur.col < len {
                    Position {
                        line: cur.line,
                        col: cur.col + 1,
                    }
                } else if cur.line + 1 < ed.line_count() {
                    Position {
                        line: cur.line + 1,
                        col: 0,
                    }
                } else {
                    cur
                }
            })
        });
    }

    pub fn move_up(&mut self, extend: bool) {
        self.map_each_selection(|ed, sel| {
            Self::move_selection(sel, extend, |cur| {
                if cur.line > 0 {
                    let line = cur.line - 1;
                    Position {
                        line,
                        col: cur.col.min(ed.line_char_len(line)),
                    }
                } else {
                    cur
                }
            })
        });
    }

    pub fn move_down(&mut self, extend: bool) {
        self.map_each_selection(|ed, sel| {
            Self::move_selection(sel, extend, |cur| {
                if cur.line + 1 < ed.line_count() {
                    let line = cur.line + 1;
                    Position {
                        line,
                        col: cur.col.min(ed.line_char_len(line)),
                    }
                } else {
                    cur
                }
            })
        });
    }

    pub fn move_home(&mut self, extend: bool) {
        self.map_each_selection(|_ed, sel| {
            Self::move_selection(sel, extend, |cur| Position {
                line: cur.line,
                col: 0,
            })
        });
    }

    pub fn move_end(&mut self, extend: bool) {
        self.map_each_selection(|ed, sel| {
            Self::move_selection(sel, extend, |cur| Position {
                line: cur.line,
                col: ed.line_char_len(cur.line),
            })
        });
    }

    pub fn move_page_up(&mut self, page: usize, extend: bool) {
        self.map_each_selection(|ed, sel| {
            Self::move_selection(sel, extend, |cur| {
                let line = cur.line.saturating_sub(page.max(1));
                Position {
                    line,
                    col: cur.col.min(ed.line_char_len(line)),
                }
            })
        });
    }

    pub fn move_page_down(&mut self, page: usize, extend: bool) {
        self.map_each_selection(|ed, sel| {
            Self::move_selection(sel, extend, |cur| {
                let max_line = ed.line_count().saturating_sub(1);
                let line = (cur.line + page.max(1)).min(max_line);
                Position {
                    line,
                    col: cur.col.min(ed.line_char_len(line)),
                }
            })
        });
    }

    /// Busca `query` desde después del cursor primario, con vuelta al inicio
    /// si no aparece más adelante. Descarta los cursores extra: es un salto,
    /// no una operación multi-selección.
    ///
    /// Recorre el rope directamente (`chars_at`, por trozos) en vez de volcar
    /// todo el buffer a un `String` primero — en un archivo grande, buscar ya
    /// implica leerlo entero (no hay forma de evitar eso), pero no hace falta
    /// además *copiarlo* entero a memoria nueva en cada búsqueda.
    pub fn find_next(&mut self, query: &str) -> bool {
        if query.is_empty() || self.rope.len_chars() == 0 {
            return false;
        }
        let qchars: Vec<char> = query.chars().collect();
        let cur_idx = self.pos_to_char_idx(self.cursor);
        let total = self.rope.len_chars();
        let start = (cur_idx + 1).min(total);

        if let Some(idx) = self.find_from(start, &qchars) {
            self.set_cursor_from_char_idx(idx);
            return true;
        }
        if let Some(idx) = self.find_from(0, &qchars) {
            self.set_cursor_from_char_idx(idx);
            return true;
        }
        false
    }

    /// Primera aparición de `qchars` en o después de `from_char`, con una
    /// ventana deslizante sobre `chars_at` — sin materializar el resto del
    /// buffer.
    fn find_from(&self, from_char: usize, qchars: &[char]) -> Option<usize> {
        let qlen = qchars.len();
        let total = self.rope.len_chars();
        if qlen == 0 || from_char + qlen > total {
            return None;
        }
        let mut chars = self.rope.chars_at(from_char);
        let mut window: std::collections::VecDeque<char> =
            (0..qlen).filter_map(|_| chars.next()).collect();
        let mut pos = from_char;
        loop {
            if window.iter().eq(qchars.iter()) {
                return Some(pos);
            }
            let next_c = chars.next()?;
            window.pop_front();
            window.push_back(next_c);
            pos += 1;
        }
    }

    /// Reemplaza todas las apariciones de `search` por `with`. Construir el
    /// documento nuevo entero es trabajo inevitable (cualquier reemplazo corre
    /// todo lo que viene después), pero antes se hacía en pasadas separadas
    /// (contar, después reemplazar) — acá se cuenta y arma el resultado en una
    /// sola pasada sobre el texto.
    pub fn replace_all(&mut self, search: &str, with: &str) -> usize {
        if search.is_empty() {
            return 0;
        }
        let text = self.rope.to_string();
        let mut new_text = String::with_capacity(text.len());
        let mut count = 0usize;
        let mut last_end = 0;
        for (start, _) in text.match_indices(search) {
            new_text.push_str(&text[last_end..start]);
            new_text.push_str(with);
            last_end = start + search.len();
            count += 1;
        }
        if count == 0 {
            return 0;
        }
        new_text.push_str(&text[last_end..]);
        self.checkpoint(EditKind::Other);
        self.rope = Rope::from_str(&new_text);
        self.cursor = Position { line: 0, col: 0 };
        self.selection_anchor = None;
        self.secondary.clear();
        self.mark_changed();
        self.needs_full_lsp_sync = true;
        self.pending_lsp_edits.clear();
        count
    }

    /// Como `find_next`, pero `pattern` es una expresión regular (sintaxis
    /// del crate `regex`). `Err` con el mensaje del motor si `pattern` no
    /// compila. A diferencia de la búsqueda literal, esta sí necesita todo
    /// el buffer como un `String` — el motor de regex no sabe leer un rope
    /// por trozos — pero eso solo pasa cuando se pide de verdad una
    /// búsqueda con regex (una acción explícita, no algo que corra en cada
    /// tecla), así que el costo es aceptable.
    pub fn find_next_regex(&mut self, pattern: &str) -> Result<bool, String> {
        let re = regex_de_editor(pattern)?;
        let text = self.rope.to_string();
        if text.is_empty() {
            return Ok(false);
        }
        let cur_idx = self.pos_to_char_idx(self.cursor);
        let start_char = (cur_idx + 1).min(self.rope.len_chars());
        let start_byte = self.rope.char_to_byte(start_char);

        // `find_at` busca la primera coincidencia en o después de
        // `start_byte` tratando `text` entero como el texto de referencia
        // (no un slice aparte) — así `^`/`$` siguen significando "principio/
        // fin de línea real", no "principio/fin de lo que quedaba del buffer".
        if let Some(m) = re.find_at(&text, start_byte) {
            let char_idx = self.rope.byte_to_char(m.start());
            self.set_cursor_from_char_idx(char_idx);
            return Ok(true);
        }
        if let Some(m) = re.find(&text) {
            let char_idx = self.rope.byte_to_char(m.start());
            self.set_cursor_from_char_idx(char_idx);
            return Ok(true);
        }
        Ok(false)
    }

    /// Como `replace_all`, pero `pattern` es una expresión regular; `with`
    /// puede referirse a grupos capturados (`$1`, `${nombre}`, sintaxis del
    /// crate `regex`). `Err` con el mensaje del motor si `pattern` no
    /// compila.
    pub fn replace_all_regex(&mut self, pattern: &str, with: &str) -> Result<usize, String> {
        let re = regex_de_editor(pattern)?;
        let text = self.rope.to_string();
        let count = re.find_iter(&text).count();
        if count == 0 {
            return Ok(0);
        }
        let new_text = re.replace_all(&text, with);
        self.checkpoint(EditKind::Other);
        self.rope = Rope::from_str(&new_text);
        self.cursor = Position { line: 0, col: 0 };
        self.selection_anchor = None;
        self.secondary.clear();
        self.mark_changed();
        self.needs_full_lsp_sync = true;
        self.pending_lsp_edits.clear();
        Ok(count)
    }
}


/// Tests de la aritmética de columnas y de indentación: lógica pura, sin
/// terminal de por medio. Es justo la parte que se rompe callada — un cursor
/// una columna corrida no tira ningún error, solo se ve mal — así que es
/// donde una prueba automática paga más que la verificación a mano.
#[cfg(test)]
mod tests {
    use super::*;

    /// Un buffer en memoria con este contenido, sin tocar el disco.
    fn ed(text: &str) -> Editor {
        let mut e = Editor::open(None).expect("un buffer sin archivo no toca el disco");
        e.rope = Rope::from_str(text);
        e
    }

    fn sel(anchor: (usize, usize), cursor: (usize, usize)) -> Selection {
        Selection {
            anchor: Position { line: anchor.0, col: anchor.1 },
            cursor: Position { line: cursor.0, col: cursor.1 },
        }
    }

    #[test]
    fn el_tabulador_se_estira_hasta_la_proxima_parada() {
        assert_eq!(char_display_width('\t', 0, 4), 4);
        assert_eq!(char_display_width('\t', 1, 4), 3);
        assert_eq!(char_display_width('\t', 3, 4), 1);
        assert_eq!(char_display_width('\t', 4, 4), 4);
    }

    #[test]
    fn no_todos_los_caracteres_miden_una_columna() {
        assert_eq!(char_display_width('a', 0, 4), 1);
        assert_eq!(char_display_width('日', 0, 4), 2);
        assert_eq!(char_display_width('😀', 0, 4), 2);
        // Un acento combinante se pinta sobre la letra anterior: cero columnas.
        assert_eq!(char_display_width('\u{301}', 0, 4), 0);
    }

    #[test]
    fn display_col_cuenta_columnas_no_caracteres() {
        let e = ed("a\tb\n日本語x\n");
        assert_eq!(e.display_col(0, 1), 1);
        assert_eq!(e.display_col(0, 2), 4, "el tabulador llega a la columna 4");
        assert_eq!(e.display_col(0, 3), 5);
        assert_eq!(e.display_col(0, e.line_char_len(0)), 5);
        assert_eq!(e.display_col(1, 3), 6, "tres caracteres CJK son seis columnas");
        assert_eq!(e.display_col(1, e.line_char_len(1)), 7);
    }

    #[test]
    fn col_from_display_es_el_inverso_y_nunca_cae_a_la_mitad() {
        let e = ed("a\tb\n日本語x\n");
        assert_eq!(e.col_from_display(0, 0), 0);
        assert_eq!(e.col_from_display(0, 2), 1, "un clic dentro del tabulador da el tabulador");
        assert_eq!(e.col_from_display(0, 4), 2);
        assert_eq!(e.col_from_display(0, 99), 3, "pasado el final, el final de la línea");
        // La mitad derecha de un carácter ancho devuelve ese carácter: no hay
        // una posición del buffer a mitad de camino donde poner el cursor.
        assert_eq!(e.col_from_display(1, 2), 1);
        assert_eq!(e.col_from_display(1, 3), 1);
        assert_eq!(e.col_from_display(1, 4), 2);
    }

    #[test]
    fn el_ajuste_de_linea_no_parte_un_caracter_ancho() {
        // Nueve CJK = 18 columnas. Con 5 de ancho entran dos por fila (4
        // columnas) y el tercero no: la fila corta antes en vez de dibujar
        // media letra.
        let e = ed("日本語日本語日本語\n");
        assert_eq!(e.wrap_row_starts(0, 5), vec![0, 4, 8, 12, 16]);
    }

    #[test]
    fn con_ancho_justo_el_corte_cae_en_el_borde() {
        let e = ed("日本語日本語\n");
        assert_eq!(e.wrap_row_starts(0, 4), vec![0, 4, 8]);
    }

    #[test]
    fn una_linea_vacia_ocupa_una_fila() {
        let e = ed("\n\n");
        assert_eq!(e.wrap_row_starts(0, 10), vec![0]);
    }

    #[test]
    fn wrap_row_of_ubica_cada_columna_en_su_fila() {
        let e = ed("日本語日本語日本語\n");
        let starts = e.wrap_row_starts(0, 5);
        assert_eq!(e.wrap_row_of(&starts, 0), 0);
        assert_eq!(e.wrap_row_of(&starts, 3), 0);
        assert_eq!(e.wrap_row_of(&starts, 4), 1);
        assert_eq!(e.wrap_row_of(&starts, 17), 4);
    }

    #[test]
    fn completa_con_palabras_del_propio_texto() {
        let mut e = ed("sincronización incremental\notra línea\n");
        e.cursor = Position { line: 1, col: 0 };
        assert_eq!(e.words_starting_with("sincr", 10), vec!["sincronización"]);
    }

    #[test]
    fn completar_del_texto_ignora_mayusculas_y_lo_ya_escrito() {
        let e = ed("Servidor y servidor\n");
        let encontradas = e.words_starting_with("SERV", 10);
        assert!(encontradas.contains(&"Servidor".to_string()));
        assert!(encontradas.contains(&"servidor".to_string()));
        // Una candidata del mismo largo que lo tipeado es lo tipeado mismo:
        // sugerirla no aportaría nada.
        assert!(e.words_starting_with("servidor", 10).is_empty());
    }

    #[test]
    fn las_sugerencias_del_texto_vienen_por_cercania_al_cursor() {
        let mut e = ed("compilacion\n\n\n\ncompilador\ncompilar\n");
        e.cursor = Position { line: 5, col: 0 };
        assert_eq!(
            e.words_starting_with("compil", 10),
            vec!["compilar", "compilador", "compilacion"],
            "lo más cerca del cursor es lo más probable que quieras repetir"
        );
    }

    #[test]
    fn indentar_no_borra_la_seleccion_ni_el_texto() {
        let mut e = ed("uno\ndos\ntres\n");
        e.apply_selections(vec![sel((0, 0), (1, 3))]);
        e.indent_lines();
        assert_eq!(e.rope.to_string(), "\tuno\n\tdos\ntres\n");
        // La selección sobrevive, así que apretar Tab de nuevo sube otro nivel
        // en vez de reemplazar el bloque.
        assert!(e.selection_spans_lines());
        e.indent_lines();
        assert_eq!(e.rope.to_string(), "\t\tuno\n\t\tdos\ntres\n");
    }

    #[test]
    fn una_seleccion_que_termina_en_la_columna_cero_no_arrastra_esa_linea() {
        let mut e = ed("uno\ndos\ntres\n");
        e.apply_selections(vec![sel((0, 0), (1, 0))]);
        e.indent_lines();
        assert_eq!(e.rope.to_string(), "\tuno\ndos\ntres\n");
    }

    #[test]
    fn indentar_con_espacios_escribe_espacios_y_corre_las_columnas() {
        let mut e = ed("uno\ndos\n");
        e.indent_with_spaces = true;
        e.tab_width = 2;
        e.apply_selections(vec![sel((0, 1), (1, 3))]);
        e.indent_lines();
        assert_eq!(e.rope.to_string(), "  uno\n  dos\n");
        // Las columnas se corren tanto como se insertó, no de a uno: si no,
        // la selección quedaría apuntando al medio de la sangría nueva.
        let s = e.selections_snapshot();
        assert_eq!((s[0].anchor.col, s[0].cursor.col), (3, 5));
    }

    #[test]
    fn des_indentar_deshace_lo_que_indento_con_espacios() {
        let mut e = ed("uno\n");
        e.indent_with_spaces = true;
        e.tab_width = 2;
        e.apply_selections(vec![sel((0, 0), (0, 3))]);
        e.indent_lines();
        e.unindent_lines();
        assert_eq!(e.rope.to_string(), "uno\n");
    }

    /// Un archivo de verdad en un directorio temporal propio de cada test,
    /// porque lo que se prueba acá es justamente la relación con el disco.
    fn archivo_temporal(nombre: &str, contenido: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint-ed-{}-{nombre}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join(nombre);
        std::fs::write(&ruta, contenido).unwrap();
        ruta
    }

    #[test]
    fn se_nota_cuando_otro_proceso_toco_el_archivo() {
        let ruta = archivo_temporal("cambia.txt", "uno\n");
        let mut e = Editor::open(Some(ruta.clone())).unwrap();
        assert!(!e.disk_changed(), "recién abierto, nada cambió");

        // Otro proceso escribe encima. La fecha tiene que quedar distinta de
        // la que Flint registró al abrir.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&ruta, "otro\n").unwrap();
        assert!(e.disk_changed(), "el archivo cambió afuera");

        // Guardar vuelve a poner de acuerdo lo de adentro con lo de afuera.
        e.dirty = true;
        e.save().unwrap();
        assert!(!e.disk_changed());
        let _ = std::fs::remove_file(ruta);
    }

    #[test]
    fn el_respaldo_se_escribe_y_se_borra_al_guardar() {
        let ruta = archivo_temporal("respaldo.txt", "uno\n");
        let mut e = Editor::open(Some(ruta.clone())).unwrap();
        let Some(destino) = e.backup_path() else {
            return; // sin HOME no hay dónde: nada que probar
        };
        let _ = std::fs::remove_file(&destino);

        // Sin cambios sin guardar no hay nada que respaldar.
        assert!(!e.write_backup_if_due(Duration::ZERO));
        e.insert_char('x');
        assert!(e.write_backup_if_due(Duration::ZERO));
        assert_eq!(std::fs::read_to_string(&destino).unwrap(), e.rope.to_string());
        // Y mientras el respaldo es más nuevo que el archivo, se avisa.
        assert!(e.pending_backup().is_some());

        e.save().unwrap();
        assert!(!destino.exists(), "guardar deja el respaldo sin razón de ser");
        assert!(e.pending_backup().is_none());
        let _ = std::fs::remove_file(ruta);
    }

    #[test]
    fn el_par_se_encuentra_en_las_dos_direcciones() {
        let e = ed("fn f(a: (u8, u8)) {}\n");
        // Desde el `(` de la firma hasta su cierre, salteando el par de
        // adentro.
        assert_eq!(
            e.matching_bracket(Position { line: 0, col: 4 }),
            Some(Position { line: 0, col: 16 })
        );
        // Y al revés, parándose sobre el de cierre.
        assert_eq!(
            e.matching_bracket(Position { line: 0, col: 16 }),
            Some(Position { line: 0, col: 4 })
        );
        // Justo después de escribir el cierre, el cursor queda una columna
        // más allá y el salto tiene que seguir funcionando.
        assert_eq!(
            e.matching_bracket(Position { line: 0, col: 17 }),
            Some(Position { line: 0, col: 4 })
        );
        assert_eq!(e.matching_bracket(Position { line: 0, col: 1 }), None);
    }

    #[test]
    fn el_par_cruza_lineas() {
        let mut e = ed("fn f() {\n    x\n}\n");
        e.goto_line(0);
        assert_eq!(
            e.matching_bracket(Position { line: 0, col: 7 }),
            Some(Position { line: 2, col: 0 })
        );
    }

    #[test]
    fn los_delimitadores_adentro_de_una_cadena_no_cuentan() {
        let mut e = ed("f(\"(\", x)\n");
        // Sin saber qué es una cadena, el `(` de adentro descuadra la cuenta
        // y no se encuentra pareja.
        assert_eq!(e.matching_bracket(Position { line: 0, col: 1 }), None);
        // Con el resaltado puesto (que es lo que hay en cuanto tree-sitter
        // corre), el de adentro se ignora y aparece el cierre de verdad.
        e.highlights_by_line = vec![vec![(2, 5, HighlightKind::String)]];
        assert_eq!(
            e.matching_bracket(Position { line: 0, col: 1 }),
            Some(Position { line: 0, col: 8 })
        );
    }

    #[test]
    fn enter_sangra_adentro_de_una_llave_abierta() {
        let mut e = ed("fn f() {\n");
        e.cursor = Position { line: 0, col: 8 };
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "fn f() {\n\t\n");
        assert_eq!(e.cursor, Position { line: 1, col: 1 });
    }

    #[test]
    fn enter_con_el_cierre_pegado_lo_baja_a_su_linea() {
        let mut e = ed("");
        e.insert_char_pairing('{');
        assert_eq!(e.rope.to_string(), "{}");
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "{\n\t\n}");
        // El cursor queda en la línea del medio, que es donde se escribe.
        assert_eq!(e.cursor, Position { line: 1, col: 1 });
    }

    #[test]
    fn enter_conserva_la_sangria_de_la_linea_y_le_suma_una() {
        let mut e = ed("    if x {\n");
        e.indent_with_spaces = true;
        e.tab_width = 4;
        e.cursor = Position { line: 0, col: 10 };
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "    if x {\n        \n");
    }

    #[test]
    fn una_llave_adentro_de_una_cadena_no_sangra() {
        let mut e = ed("let s = \"{\";\n");
        // Sin resaltado, la llave de la cadena se cuenta y sangra de más.
        e.cursor = Position { line: 0, col: 12 };
        e.highlights_by_line = vec![vec![(8, 11, HighlightKind::String)]];
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "let s = \"{\";\n\n");
    }

    #[test]
    fn una_llave_ya_cerrada_en_la_misma_linea_no_sangra() {
        let mut e = ed("f(x);\n");
        e.cursor = Position { line: 0, col: 5 };
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "f(x);\n\n");
    }

    /// Escribe `texto` letra por letra, como el usuario.
    fn tipear(e: &mut Editor, texto: &str) {
        for c in texto.chars() {
            e.insert_char_pairing(c);
        }
    }

    #[test]
    fn el_cierre_escrito_a_mano_vuelve_a_la_sangria_de_su_apertura() {
        let mut e = ed("fn f() {\n    x();\n    \n");
        e.auto_close = false;
        e.cursor = Position { line: 2, col: 4 };
        tipear(&mut e, "}");
        assert_eq!(e.rope.to_string(), "fn f() {\n    x();\n}\n");
        assert_eq!(e.cursor, Position { line: 2, col: 1 });

        // Anidado: toma la sangría de *su* apertura, no la del principio.
        let mut e = ed("a {\n    b {\n        c\n        \n");
        e.auto_close = false;
        e.cursor = Position { line: 3, col: 8 };
        tipear(&mut e, "}");
        assert_eq!(e.rope.to_string(), "a {\n    b {\n        c\n    }\n");

        // Paréntesis y corchetes también.
        let mut e = ed("f(\n    1,\n    \n");
        e.auto_close = false;
        e.cursor = Position { line: 2, col: 4 };
        tipear(&mut e, ")");
        assert_eq!(e.rope.to_string(), "f(\n    1,\n)\n");
    }

    #[test]
    fn el_cierre_no_toca_la_sangria_cuando_no_corresponde() {
        // Con texto antes del cierre en la misma línea.
        let mut e = ed("f {\n    x \n");
        e.auto_close = false;
        e.cursor = Position { line: 1, col: 6 };
        tipear(&mut e, "}");
        assert_eq!(e.rope.to_string(), "f {\n    x }\n");

        // Sin apertura que le haga pareja.
        let mut e = ed("x\n    \n");
        e.cursor = Position { line: 1, col: 4 };
        tipear(&mut e, "}");
        assert_eq!(e.rope.to_string(), "x\n    }\n");

        // Ya estaba menos sangrado que la apertura: no se agrega sangría.
        let mut e = ed("    f {\n  \n");
        e.auto_close = false;
        e.cursor = Position { line: 1, col: 2 };
        tipear(&mut e, "}");
        assert_eq!(e.rope.to_string(), "    f {\n  }\n");
    }

    #[test]
    fn deshacer_devuelve_la_sangria_y_deja_el_cierre() {
        let mut e = ed("f {\n    \n");
        e.auto_close = false;
        e.cursor = Position { line: 1, col: 4 };
        tipear(&mut e, "}");
        assert_eq!(e.rope.to_string(), "f {\n}\n");
        e.undo();
        assert_eq!(e.rope.to_string(), "f {\n    }\n");
    }

    #[test]
    fn en_python_else_elif_except_y_finally_vuelven_a_su_bloque() {
        let py = |texto: &str, linea: usize, col: usize, tipeo: &str| {
            let mut e = ed(texto);
            e.indent_after_colon = true;
            e.auto_close = false;
            e.cursor = Position { line: linea, col };
            tipear(&mut e, tipeo);
            e.rope.to_string()
        };
        assert_eq!(py("if a:\n    x\n    \n", 2, 4, "else:"), "if a:\n    x\nelse:\n");
        assert_eq!(py("if a:\n    x\n    \n", 2, 4, "elif b:"), "if a:\n    x\nelif b:\n");
        assert_eq!(
            py("try:\n    x\n\n    # nota\n    \n", 4, 4, "except ValueError as e:"),
            "try:\n    x\n\n    # nota\nexcept ValueError as e:\n"
        );
        assert_eq!(py("try:\n    x\nexcept:\n    y\n    \n", 4, 4, "finally:"), "try:\n    x\nexcept:\n    y\nfinally:\n");
        // Anidado: vuelve al `if` de adentro, no al de afuera.
        assert_eq!(
            py("if a:\n    if b:\n        x\n        \n", 3, 8, "else:"),
            "if a:\n    if b:\n        x\n    else:\n"
        );
        // El bloque de arriba no admite un else: no se toca.
        assert_eq!(py("def f():\n    x\n    \n", 2, 4, "else:"), "def f():\n    x\n    else:\n");
        // Otras palabras no des-indentan.
        assert_eq!(py("if a:\n    x\n    \n", 2, 4, "elsewhere:"), "if a:\n    x\n    elsewhere:\n");
    }

    #[test]
    fn fuera_de_python_los_dos_puntos_no_desindentan() {
        let mut e = ed("if a {\n    x\n    \n");
        e.auto_close = false;
        e.cursor = Position { line: 2, col: 4 };
        tipear(&mut e, "else:");
        assert_eq!(e.rope.to_string(), "if a {\n    x\n    else:\n");
    }

    #[test]
    fn en_python_los_dos_puntos_abren_bloque() {
        let mut e = ed("def f():\n");
        e.indent_after_colon = true;
        e.indent_with_spaces = true;
        e.tab_width = 4;
        e.cursor = Position { line: 0, col: 8 };
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "def f():\n    \n");

        // En un lenguaje con llaves, un `:` al final es otra cosa.
        let mut e = ed("case x:\n");
        e.cursor = Position { line: 0, col: 7 };
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "case x:\n\n");
    }

    #[test]
    fn con_varios_cursores_enter_sigue_copiando_la_sangria_de_cada_linea() {
        let mut e = ed("  uno\n    dos\n");
        e.apply_selections(vec![sel((0, 5), (0, 5)), sel((1, 7), (1, 7))]);
        e.insert_newline();
        assert_eq!(e.rope.to_string(), "  uno\n  \n    dos\n    \n");
    }

    #[test]
    fn el_cierre_automatico_pone_el_par_y_deja_el_cursor_adentro() {
        let mut e = ed("");
        e.insert_char_pairing('(');
        assert_eq!(e.rope.to_string(), "()");
        assert_eq!(e.cursor.col, 1);
        // Escribir el cierre que ya está no lo duplica: se pasa por encima.
        e.insert_char_pairing(')');
        assert_eq!(e.rope.to_string(), "()");
        assert_eq!(e.cursor.col, 2);
    }

    #[test]
    fn el_cierre_automatico_no_se_mete_en_medio_de_una_palabra() {
        let mut e = ed("hola\n");
        e.cursor = Position { line: 0, col: 0 };
        e.insert_char_pairing('(');
        assert_eq!(e.rope.to_string(), "(hola\n", "pegado a texto no se cierra");

        // Un apóstrofo en medio de una palabra tampoco abre una comilla.
        let mut e = ed("dont\n");
        e.cursor = Position { line: 0, col: 4 };
        e.insert_char_pairing('\'');
        assert_eq!(e.rope.to_string(), "dont'\n");
    }

    #[test]
    fn backspace_entre_un_par_vacio_se_lleva_los_dos() {
        let mut e = ed("");
        e.insert_char_pairing('[');
        assert!(e.backspace_pair());
        assert_eq!(e.rope.to_string(), "");
        // Con algo adentro ya no es un par vacío: Backspace borra una sola
        // cosa, como siempre.
        e.insert_char_pairing('[');
        e.insert_char('x');
        assert!(!e.backspace_pair());
    }

    #[test]
    fn comentar_usa_la_sangria_minima_del_bloque() {
        let mut e = ed("fn f() {\n    let x = 1;\n        let y = 2;\n}\n");
        e.apply_selections(vec![sel((1, 0), (2, 5))]);
        assert!(e.toggle_line_comment("//"));
        // El token entra a la altura de la línea menos sangrada, así que la
        // escalera de adentro del bloque se conserva.
        assert_eq!(
            e.rope.to_string(),
            "fn f() {\n    // let x = 1;\n    //     let y = 2;\n}\n"
        );
    }

    #[test]
    fn descomentar_deja_el_texto_como_estaba() {
        let mut e = ed("    // uno\n    // dos\n");
        e.apply_selections(vec![sel((0, 0), (1, 5))]);
        assert!(e.toggle_line_comment("//"));
        assert_eq!(e.rope.to_string(), "    uno\n    dos\n");
    }

    #[test]
    fn un_bloque_a_medio_comentar_se_comenta_entero() {
        let mut e = ed("// uno\ndos\n");
        e.apply_selections(vec![sel((0, 0), (1, 3))]);
        e.toggle_line_comment("//");
        assert_eq!(e.rope.to_string(), "// // uno\n// dos\n");
    }

    #[test]
    fn comentar_saltea_las_lineas_en_blanco() {
        let mut e = ed("uno\n\ndos\n");
        e.apply_selections(vec![sel((0, 0), (2, 3))]);
        e.toggle_line_comment("#");
        assert_eq!(e.rope.to_string(), "# uno\n\n# dos\n");
        // Y una selección de puro blanco no gasta un paso de deshacer.
        let mut vacio = ed("\n\n");
        vacio.apply_selections(vec![sel((0, 0), (1, 0))]);
        assert!(!vacio.toggle_line_comment("#"));
    }

    #[test]
    fn ir_a_una_linea_de_mas_cae_en_la_ultima() {
        let mut e = ed("uno\ndos\ntres\n");
        e.goto_line(1);
        assert_eq!(e.cursor, Position { line: 1, col: 0 });
        e.goto_line(9999);
        assert_eq!(e.cursor.line, e.line_count() - 1);
    }

    #[test]
    fn el_recorte_saca_el_espacio_final_y_deja_el_resto() {
        let mut e = ed("uno   \n  dos\t\n\ntres\n");
        assert!(e.trim_trailing_whitespace());
        assert_eq!(e.rope.to_string(), "uno\n  dos\n\ntres\n");
        // La segunda pasada no tiene nada que hacer, y decirlo es lo que
        // evita gastar un paso de deshacer al guardar un archivo ya limpio.
        assert!(!e.trim_trailing_whitespace());
    }

    #[test]
    fn el_recorte_trae_al_cursor_de_vuelta_al_final_de_su_linea() {
        let mut e = ed("uno    \n");
        e.apply_selections(vec![sel((0, 7), (0, 7))]);
        e.trim_trailing_whitespace();
        assert_eq!(e.cursor.col, 3, "el cursor no puede quedar fuera de la línea");
    }

    #[test]
    fn des_indentar_respeta_con_que_esta_indentada_cada_linea() {
        let mut e = ed("\tcon tab\n        con espacios\nsin nada\n");
        e.apply_selections(vec![sel((0, 0), (2, 3))]);
        e.unindent_lines();
        assert_eq!(e.rope.to_string(), "con tab\n    con espacios\nsin nada\n");
    }

    #[test]
    fn des_indentar_sin_nada_que_sacar_no_ensucia_el_buffer() {
        let mut e = ed("sin nada\n");
        e.unindent_lines();
        assert!(!e.dirty, "no se tocó el buffer: no debería quedar marcado como sucio");
        assert_eq!(e.rope.to_string(), "sin nada\n");
    }
}

#[cfg(test)]
mod tests_palabra_bajo_cursor {
    use super::*;

    fn en(texto: &str, line: usize, col: usize) -> String {
        let mut e = Editor::open(None).expect("editor vacío");
        e.rope = Rope::from_str(texto);
        e.cursor = Position { line, col };
        e.word_under_cursor()
    }

    #[test]
    fn agarra_la_palabra_este_donde_este_el_cursor() {
        let src = "let cuenta_total = 1;\n";
        // Al principio, en el medio y justo al final de la palabra.
        assert_eq!(en(src, 0, 4), "cuenta_total");
        assert_eq!(en(src, 0, 9), "cuenta_total");
        assert_eq!(en(src, 0, 16), "cuenta_total");
        // Y la de la izquierda cuando el cursor quedó pegado al final.
        assert_eq!(en(src, 0, 3), "let");
    }

    #[test]
    fn sin_palabra_devuelve_vacio() {
        assert_eq!(en("  +  \n", 0, 2), "");
        assert_eq!(en("\n", 0, 0), "");
        // Más allá del final de la línea.
        assert_eq!(en("ab\n", 0, 99), "ab");
    }

    #[test]
    fn los_acentos_y_el_guion_bajo_son_parte_de_la_palabra() {
        assert_eq!(en("let año_1 = 2;\n", 0, 5), "año_1");
    }
}

/// El ciclo de edición completo: deshacer/rehacer, multi-cursor y buscar y
/// reemplazar. Hasta acá se probaba a mano, y es lo que más fácil se rompe
/// sin que nada avise: un cursor que queda una letra corrida o un deshacer
/// que se lleva de más no tiran ningún error.
#[cfg(test)]
mod tests_ciclo_de_edicion {
    use super::*;

    fn ed(texto: &str) -> Editor {
        let mut e = Editor::open(None).expect("un buffer sin archivo no toca el disco");
        e.rope = Rope::from_str(texto);
        e
    }

    fn pos(line: usize, col: usize) -> Position {
        Position { line, col }
    }

    fn cursores(e: &mut Editor, lugares: &[(usize, usize)]) {
        let sels = lugares.iter().map(|&(l, c)| Selection::point(pos(l, c))).collect();
        e.apply_selections(sels);
    }

    fn escribir(e: &mut Editor, texto: &str) {
        for c in texto.chars() {
            e.insert_char(c);
        }
    }

    /// Hace de cuenta que pasó el margen que agrupa las ediciones seguidas,
    /// sin esperarlo de verdad.
    fn pasa_el_tiempo(e: &mut Editor) {
        e.last_edit_at = e.last_edit_at.map(|t| t - GROUP_TIMEOUT - Duration::from_millis(1));
    }

    fn texto(e: &Editor) -> String {
        e.rope.to_string()
    }

    fn puntos(e: &Editor) -> Vec<Position> {
        e.selections_snapshot().iter().map(|s| s.cursor).collect()
    }

    // ---------- deshacer y rehacer ----------

    #[test]
    fn escribir_seguido_se_deshace_de_una_vez() {
        let mut e = ed("");
        escribir(&mut e, "hola");
        assert!(e.undo());
        assert_eq!(texto(&e), "");
        assert_eq!(e.cursor, pos(0, 0));
        assert!(!e.undo(), "no quedaba nada más");
    }

    #[test]
    fn pasado_el_margen_de_tiempo_es_otro_paso() {
        let mut e = ed("");
        escribir(&mut e, "ab");
        pasa_el_tiempo(&mut e);
        escribir(&mut e, "cd");
        e.undo();
        assert_eq!(texto(&e), "ab");
        e.undo();
        assert_eq!(texto(&e), "");
    }

    #[test]
    fn escribir_y_borrar_son_pasos_distintos() {
        let mut e = ed("");
        escribir(&mut e, "abc");
        e.backspace();
        e.backspace();
        assert_eq!(texto(&e), "a");
        e.undo();
        assert_eq!(texto(&e), "abc", "deshace los dos borrados juntos");
        e.undo();
        assert_eq!(texto(&e), "");
    }

    #[test]
    fn rehacer_deja_texto_y_cursor_como_antes_de_deshacer() {
        let mut e = ed("uno\n");
        cursores(&mut e, &[(0, 3)]);
        escribir(&mut e, " dos");
        let (antes, cursor) = (texto(&e), e.cursor);
        e.undo();
        assert_eq!(e.cursor, pos(0, 3));
        assert!(e.redo());
        assert_eq!(texto(&e), antes);
        assert_eq!(e.cursor, cursor);
        assert!(!e.redo());
    }

    #[test]
    fn una_edicion_nueva_despues_de_deshacer_borra_lo_que_habia_para_rehacer() {
        let mut e = ed("");
        escribir(&mut e, "ab");
        e.undo();
        escribir(&mut e, "x");
        assert!(!e.redo());
        assert_eq!(texto(&e), "x");
    }

    #[test]
    fn deshacer_sin_nada_no_ensucia_el_buffer() {
        let mut e = ed("intacto");
        let version = e.content_version;
        assert!(!e.undo());
        assert!(!e.redo());
        assert!(!e.dirty);
        assert_eq!(e.content_version, version);
    }

    #[test]
    fn deshacer_devuelve_todos_los_cursores_a_donde_estaban() {
        let mut e = ed("a\nb\nc\n");
        cursores(&mut e, &[(0, 1), (1, 1), (2, 1)]);
        escribir(&mut e, "!");
        assert_eq!(texto(&e), "a!\nb!\nc!\n");
        e.undo();
        assert_eq!(texto(&e), "a\nb\nc\n");
        assert_eq!(puntos(&e), vec![pos(0, 1), pos(1, 1), pos(2, 1)]);
    }

    #[test]
    fn la_pila_de_deshacer_tiene_tope_y_se_pierden_los_mas_viejos() {
        let mut e = ed("");
        for _ in 0..MAX_UNDO + 5 {
            e.insert_char('x');
            pasa_el_tiempo(&mut e);
        }
        let mut deshechos = 0;
        while e.undo() {
            deshechos += 1;
        }
        assert_eq!(deshechos, MAX_UNDO);
        assert_eq!(texto(&e), "xxxxx", "quedan los cinco que no entraron");
    }

    #[test]
    fn deshacer_pide_mandarle_el_documento_entero_al_servidor() {
        let mut e = ed("");
        escribir(&mut e, "ab");
        let _ = e.take_lsp_sync_plan();
        e.undo();
        assert!(matches!(e.take_lsp_sync_plan(), LspSyncPlan::Full));
    }

    // ---------- multi-cursor ----------

    #[test]
    fn escribir_con_varios_cursores_en_la_misma_linea() {
        let mut e = ed("ab ab ab");
        cursores(&mut e, &[(0, 2), (0, 5), (0, 8)]);
        escribir(&mut e, "XY");
        assert_eq!(texto(&e), "abXY abXY abXY");
        assert_eq!(puntos(&e), vec![pos(0, 4), pos(0, 9), pos(0, 14)]);
    }

    #[test]
    fn el_orden_en_que_se_agregaron_los_cursores_no_cambia_el_resultado() {
        let mut e = ed("ab ab ab");
        // La primaria es la de la derecha: las ediciones igual se aplican
        // de atrás para adelante y cada cursor termina detrás de lo suyo.
        cursores(&mut e, &[(0, 8), (0, 2), (0, 5)]);
        escribir(&mut e, "X");
        assert_eq!(texto(&e), "abX abX abX");
        assert_eq!(puntos(&e), vec![pos(0, 11), pos(0, 3), pos(0, 7)]);
    }

    #[test]
    fn borrar_con_varios_cursores_puede_unir_lineas() {
        let mut e = ed("ab\ncd\nef");
        cursores(&mut e, &[(1, 0), (2, 0)]);
        e.backspace();
        assert_eq!(texto(&e), "abcdef");
        assert_eq!(puntos(&e), vec![pos(0, 2), pos(0, 4)]);
    }

    #[test]
    fn suprimir_con_varios_cursores() {
        let mut e = ed("xa xb xc");
        cursores(&mut e, &[(0, 0), (0, 3), (0, 6)]);
        e.delete_forward();
        assert_eq!(texto(&e), "a b c");
        assert_eq!(puntos(&e), vec![pos(0, 0), pos(0, 2), pos(0, 4)]);
    }

    #[test]
    fn un_cursor_al_principio_del_archivo_no_borra_nada_ni_desalinea_a_los_otros() {
        let mut e = ed("abc\ndef");
        cursores(&mut e, &[(0, 0), (1, 3)]);
        e.backspace();
        assert_eq!(texto(&e), "abc\nde");
        assert_eq!(puntos(&e), vec![pos(0, 0), pos(1, 2)]);
    }

    #[test]
    fn seleccionar_las_siguientes_apariciones_y_reemplazarlas_escribiendo() {
        let mut e = ed("foo bar foo baz foo");
        e.apply_selections(vec![Selection { anchor: pos(0, 0), cursor: pos(0, 3) }]);
        assert!(e.select_next_occurrence());
        assert!(e.select_next_occurrence());
        assert!(!e.select_next_occurrence(), "ya estaban las tres");
        e.insert_char('X');
        assert_eq!(texto(&e), "X bar X baz X");
        assert!(e.undo());
        assert_eq!(texto(&e), "foo bar foo baz foo", "un solo paso para todas");
    }

    #[test]
    fn la_siguiente_aparicion_da_la_vuelta_al_archivo() {
        let mut e = ed("uno dos uno");
        // Se parte de la segunda: la que falta está antes.
        e.apply_selections(vec![Selection { anchor: pos(0, 8), cursor: pos(0, 11) }]);
        assert!(e.select_next_occurrence());
        let rangos: Vec<(usize, usize)> =
            e.selections_snapshot().iter().map(|s| e.selection_char_range(*s)).collect();
        assert_eq!(rangos, vec![(8, 11), (0, 3)]);
    }

    #[test]
    fn mover_con_shift_extiende_la_seleccion_de_cada_cursor() {
        let mut e = ed("abcd\nefgh");
        cursores(&mut e, &[(0, 0), (1, 0)]);
        e.move_right(true);
        e.move_right(true);
        assert_eq!(e.selected_text().as_deref(), Some("ab\nef"));
        e.backspace();
        assert_eq!(texto(&e), "cd\ngh");
    }

    #[test]
    fn cursores_que_terminan_en_el_mismo_lugar_se_funden() {
        // Dos cursores pegados que borran hacia atrás quedan los dos en la
        // columna 0. Si siguieran siendo dos, la próxima letra se escribiría
        // dos veces.
        let mut e = ed("abc");
        cursores(&mut e, &[(0, 1), (0, 2)]);
        e.backspace();
        assert_eq!(texto(&e), "c");
        escribir(&mut e, "X");
        assert_eq!(texto(&e), "Xc");
        assert_eq!(puntos(&e), vec![pos(0, 1)]);
    }

    #[test]
    fn selecciones_que_se_pisan_borran_la_union_y_no_de_mas() {
        // Una selección de 0 a 3 y otro cursor adentro de ella, en la 2.
        let mut e = ed("abcdef");
        e.apply_selections(vec![
            Selection { anchor: pos(0, 0), cursor: pos(0, 3) },
            Selection::point(pos(0, 2)),
        ]);
        e.backspace();
        assert_eq!(texto(&e), "def");
        assert_eq!(puntos(&e), vec![pos(0, 0)]);
    }

    #[test]
    fn con_un_cursor_al_servidor_le_llegan_deltas_y_con_varios_el_documento() {
        let mut e = ed("ab\ncd");
        let _ = e.take_lsp_sync_plan();
        cursores(&mut e, &[(0, 2)]);
        escribir(&mut e, "!");
        match e.take_lsp_sync_plan() {
            LspSyncPlan::Incremental(cambios) => {
                assert_eq!(cambios.len(), 1);
                assert_eq!((cambios[0].start_line, cambios[0].start_col_utf16), (0, 2));
                assert_eq!(cambios[0].text, "!");
            }
            _ => panic!("un cursor escribiendo tiene que ir como delta"),
        }
        cursores(&mut e, &[(0, 0), (1, 0)]);
        escribir(&mut e, "#");
        assert!(matches!(e.take_lsp_sync_plan(), LspSyncPlan::Full));
    }

    // ---------- buscar y reemplazar ----------

    #[test]
    fn buscar_avanza_y_da_la_vuelta() {
        let mut e = ed("gato\nperro gato\ngato");
        assert!(e.find_next("gato"), "el cursor está sobre la primera: salta a la siguiente");
        assert_eq!(e.cursor, pos(1, 6));
        assert!(e.find_next("gato"));
        assert_eq!(e.cursor, pos(2, 0));
        assert!(e.find_next("gato"));
        assert_eq!(e.cursor, pos(0, 0), "da la vuelta");
    }

    #[test]
    fn buscar_algo_que_no_esta_no_mueve_el_cursor() {
        let mut e = ed("abc\ndef");
        cursores(&mut e, &[(1, 2)]);
        assert!(!e.find_next("xyz"));
        assert!(!e.find_next(""));
        assert_eq!(e.cursor, pos(1, 2));
    }

    #[test]
    fn buscar_cuenta_columnas_en_caracteres() {
        let mut e = ed("ñandú café\ncafé");
        assert!(e.find_next("café"));
        assert_eq!(e.cursor, pos(0, 6));
        assert!(e.find_next("é"));
        assert_eq!(e.cursor, pos(0, 9));
    }

    #[test]
    fn reemplazar_todo_cuenta_y_se_deshace_de_una() {
        let mut e = ed("a-b-c-d");
        assert_eq!(e.replace_all("-", " + "), 3);
        assert_eq!(texto(&e), "a + b + c + d");
        assert!(e.dirty);
        assert!(e.undo());
        assert_eq!(texto(&e), "a-b-c-d");
    }

    #[test]
    fn reemplazar_por_algo_que_contiene_lo_buscado_no_se_repite() {
        let mut e = ed("a a");
        assert_eq!(e.replace_all("a", "aa"), 2);
        assert_eq!(texto(&e), "aa aa");
    }

    #[test]
    fn reemplazar_sin_coincidencias_no_ensucia_ni_gasta_un_paso() {
        let mut e = ed("nada que ver");
        assert_eq!(e.replace_all("xyz", "w"), 0);
        assert_eq!(e.replace_all("", "w"), 0);
        assert!(!e.dirty);
        assert!(!e.undo());
    }

    #[test]
    fn reemplazar_todo_deja_los_cursores_dentro_del_texto_nuevo() {
        let mut e = ed("una linea bastante larga\notra");
        cursores(&mut e, &[(0, 20), (1, 4)]);
        e.replace_all("bastante larga", "corta");
        assert_eq!(texto(&e), "una linea corta\notra");
        for p in puntos(&e) {
            assert!(p.col <= e.line_char_len(p.line), "cursor fuera de la línea: {p:?}");
        }
        assert!(matches!(e.take_lsp_sync_plan(), LspSyncPlan::Full));
    }

    #[test]
    fn reemplazar_con_regex_usa_los_grupos() {
        let mut e = ed("foo123 bar45");
        assert_eq!(e.replace_all_regex(r"([a-z]+)(\d+)", "$2$1"), Ok(2));
        assert_eq!(texto(&e), "123foo 45bar");
        e.undo();
        assert_eq!(texto(&e), "foo123 bar45");
    }

    #[test]
    fn una_regex_invalida_avisa_y_no_toca_nada() {
        let mut e = ed("(abc)");
        assert!(e.replace_all_regex("(abc", "x").is_err());
        assert!(e.find_next_regex("[").is_err());
        assert_eq!(texto(&e), "(abc)");
        assert!(!e.dirty);
    }

    #[test]
    fn la_regex_entiende_principio_de_linea_aunque_se_busque_desde_el_medio() {
        let mut e = ed("x fn\nfn y\n  fn");
        assert_eq!(e.find_next_regex("^fn"), Ok(true));
        assert_eq!(e.cursor, pos(1, 0));
        assert_eq!(e.find_next_regex("^fn"), Ok(true));
        assert_eq!(e.cursor, pos(1, 0), "es la única al principio de una línea");
    }
}
