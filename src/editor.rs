use regex::Regex;
use ropey::Rope;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};
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

#[derive(Clone)]
pub struct CompletionEntry {
    pub label: String,
    pub detail: Option<String>,
    /// Texto a insertar si se elige esta entrada, cuando el servidor lo da y
    /// no es un snippet (`$0`, `${1:nombre}`...) — Flint no expande snippets
    /// todavía, así que en ese caso se usa `label` en cambio.
    pub insert_text: Option<String>,
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
    pub col_offset: usize,
    pub dirty: bool,
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
    /// Registro interno de copiar/pegar propio de Flint (todavía no habla con
    /// el portapapeles del sistema — ver README).
    pub register: Option<String>,
    /// Cada cuántas columnas cae una parada de tabulación al dibujar un `\t`.
    /// Es solo presentación: en el buffer el tabulador sigue siendo un único
    /// carácter, igual que en el archivo.
    pub tab_width: usize,
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

impl Editor {
    pub fn open(path: Option<PathBuf>) -> io::Result<Editor> {
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
            col_offset: 0,
            dirty: false,
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
            register: None,
            tab_width: DEFAULT_TAB_WIDTH,
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

    /// El identificador que ya está tipeado justo antes del cursor primario
    /// (alfanumérico+`_`, sin cruzar espacios ni puntuación) y dónde
    /// empieza. Lo usa el autocompletado para arrancar el popup ya filtrado
    /// por lo que se escribió antes de pedirlo (`s.pu` + `Ctrl+Espacio`
    /// arranca filtrado por "pu", no vacío) y para saber desde dónde
    /// reemplazar al aceptar una sugerencia.
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
        let path = self
            .filename
            .clone()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "sin nombre de archivo"))?;
        let mut file = fs::File::create(&path)?;
        for chunk in self.rope.chunks() {
            file.write_all(chunk.as_bytes())?;
        }
        self.dirty = false;
        Ok(())
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

    /// Ancho en columnas de pantalla de la línea entera, sin el terminador.
    pub fn line_display_width(&self, line: usize) -> usize {
        self.display_col(line, self.line_char_len(line))
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

        let mut sels = sels;
        for (i, char_idx) in final_char_idx {
            sels[i] = Selection::point(self.position_from_char_idx(char_idx));
        }
        self.apply_selections(sels);
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

    fn clamp_position(&self, pos: Position) -> Position {
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
        let le = self.line_ending.as_str();
        self.edit_all_selections(EditKind::Other, move |rope, start, end| {
            let line = rope.char_to_line(start.min(rope.len_chars()));
            let indent = Self::leading_whitespace(rope, line);
            (start, end, format!("{le}{indent}"))
        });
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

    /// Suma un nivel de indentación (un `\t`) al principio de cada línea que
    /// toquen las selecciones, y las conserva — así se puede apretar Tab
    /// varias veces seguidas sobre el mismo bloque, en vez de perder la
    /// selección en la primera.
    pub fn indent_lines(&mut self) {
        let lines = self.lines_touched();
        if lines.is_empty() {
            return;
        }
        self.checkpoint(EditKind::Other);
        let mut sels = self.selections_snapshot();
        // De abajo hacia arriba: insertar en una línea no corre los índices
        // de carácter de las que están más arriba.
        for &line in lines.iter().rev() {
            let at = self.rope.line_to_char(line);
            self.rope.insert_char(at, '\t');
        }
        for sel in &mut sels {
            for p in [&mut sel.anchor, &mut sel.cursor] {
                // La columna 0 se queda en 0: es donde suele estar el extremo
                // de una selección de líneas enteras, y correrla dejaría el
                // bloque seleccionado a partir del segundo carácter.
                if p.col > 0 && lines.binary_search(&p.line).is_ok() {
                    p.col += 1;
                }
            }
        }
        self.apply_selections(sels);
        self.after_multiline_edit();
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
        let re = Regex::new(pattern).map_err(|e| e.to_string())?;
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
        let re = Regex::new(pattern).map_err(|e| e.to_string())?;
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
        assert_eq!(e.line_display_width(0), 5);
        assert_eq!(e.display_col(1, 3), 6, "tres caracteres CJK son seis columnas");
        assert_eq!(e.line_display_width(1), 7);
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
