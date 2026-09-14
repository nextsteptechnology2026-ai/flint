use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;

/// Dónde entrar a INSERT desde la capa modal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InsertAt {
    SelectionStart,
    SelectionEnd,
    LineStart,
    LineEnd,
}

/// Cada cosa que Flint sabe hacer en respuesta a una tecla, con nombre
/// estable — esto es lo que un perfil de teclado (o un remapeo propio)
/// asigna a una combinación de teclas concreta.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Save,
    Quit,
    SearchPrompt,
    ReplacePrompt,
    SearchRegexPrompt,
    ReplaceRegexPrompt,
    SystemCopy,
    SystemCut,
    SystemPaste,
    Undo,
    Redo,
    JumpDiagnostic,
    TriggerCompletion,
    ToggleModalLayer,
    SelectNextOccurrence,
    CommandPalette,
    OpenFilePrompt,
    NextBuffer,
    PrevBuffer,
    CloseBuffer,
    ToggleWrap,
    TogglePreview,
    MoveLeft(bool),
    MoveRight(bool),
    MoveUp(bool),
    MoveDown(bool),
    MoveHome(bool),
    MoveEnd(bool),
    PageUp(bool),
    PageDown(bool),
    InsertNewline,
    Backspace,
    DeleteForward,
    InsertTab,
    Unindent,
    SelectWord,
    SelectLine,
    ExpandSelection,
    NormalDelete,
    NormalChange,
    NormalYank,
    NormalPaste,
    InsertAt(InsertAt),
    OpenBelow,
    OpenAbove,
    EscapeKey,
    /// Comentar o descomentar las líneas de la selección.
    ToggleComment,
    /// Preguntar a qué línea saltar.
    GotoLinePrompt,
    /// Saltar a donde está definido el símbolo bajo el cursor (LSP).
    GotoDefinition,
    /// Renombrar el símbolo bajo el cursor en todo el proyecto (LSP).
    RenamePrompt,
    /// Abrir el buscador difuso de archivos del proyecto.
    FindFilePrompt,
    /// Buscar texto en todos los archivos del proyecto.
    SearchProjectPrompt,
    /// Saltar al delimitador que hace pareja con el de al lado del cursor.
    JumpMatchingBracket,
    /// Empezar a grabar una macro, o terminarla si ya se está grabando.
    MacroRecord,
    /// Repetir la última macro grabada.
    MacroPlay,
    /// Escribir un carácter. No se puede escribir en la configuración (no
    /// tiene nombre): existe para que *todo* lo que hace una tecla pase por
    /// el despachador de acciones, que es lo que permite que una macro
    /// grabe también lo tipeado y no solo los atajos.
    InsertChar(char),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KeyToken {
    Char(char),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Tab,
    Backspace,
    Delete,
    Esc,
    /// Teclas de función. F2 no llega nunca acá: es el interruptor de la
    /// capa modal y se atiende antes que el keymap.
    F(u8),
}

/// Una combinación de teclas concreta. Para `Char`, mayúscula/minúscula ya
/// viene distinguida en el propio carácter (así llega de crossterm), así que
/// `shift` solo se usa para teclas como las flechas, que no cambian de
/// "código" al mantener Shift — solo el modificador.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KeyChord {
    pub token: KeyToken,
    pub ctrl: bool,
    pub shift: bool,
}

impl KeyChord {
    const fn plain(token: KeyToken) -> Self {
        KeyChord {
            token,
            ctrl: false,
            shift: false,
        }
    }
    const fn with_shift(token: KeyToken) -> Self {
        KeyChord {
            token,
            ctrl: false,
            shift: true,
        }
    }
    const fn with_ctrl(token: KeyToken) -> Self {
        KeyChord {
            token,
            ctrl: true,
            shift: false,
        }
    }
}

/// Convierte un evento real de crossterm a una `KeyChord`, o `None` si es una
/// tecla que Flint no distingue (F-keys salvo F2, que se maneja aparte).
pub fn chord_from_event(key: &KeyEvent) -> Option<KeyChord> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift_mod = key.modifiers.contains(KeyModifiers::SHIFT);
    let token = match key.code {
        KeyCode::Char(c) => KeyToken::Char(c),
        KeyCode::Left => KeyToken::Left,
        KeyCode::Right => KeyToken::Right,
        KeyCode::Up => KeyToken::Up,
        KeyCode::Down => KeyToken::Down,
        KeyCode::Home => KeyToken::Home,
        KeyCode::End => KeyToken::End,
        KeyCode::PageUp => KeyToken::PageUp,
        KeyCode::PageDown => KeyToken::PageDown,
        KeyCode::Enter => KeyToken::Enter,
        KeyCode::Tab => KeyToken::Tab,
        // Shift+Tab no llega como Tab+SHIFT: la terminal manda su propio
        // código. Se normaliza al mismo token para que quede expresado como
        // el chord "Tab con shift", igual que Shift+flecha.
        KeyCode::BackTab => KeyToken::Tab,
        KeyCode::Backspace => KeyToken::Backspace,
        KeyCode::Delete => KeyToken::Delete,
        KeyCode::Esc => KeyToken::Esc,
        KeyCode::F(n) => KeyToken::F(n),
        _ => return None,
    };
    // Para un carácter, mayúscula/minúscula ya lo distingue; no dupliques la
    // señal con el modificador, o "Shift+w" y "W" dejarían de ser la misma tecla.
    let shift = if matches!(token, KeyToken::Char(_)) {
        false
    } else {
        // `BackTab` ya *es* Shift+Tab; algunas terminales además mandan el
        // modificador y otras no, así que se da por puesto en vez de confiar
        // en que venga.
        shift_mod || key.code == KeyCode::BackTab
    };
    Some(KeyChord { token, ctrl, shift })
}

/// Lo que cuelga de una tecla en la capa directa: o bien una acción directa,
/// o un prefijo (como el `Ctrl+X` de Emacs) que espera una tecla más.
pub enum Binding {
    Do(Action),
    Prefix(HashMap<KeyChord, Action>),
}

pub struct Keymap {
    pub name: &'static str,
    pub label: &'static str,
    pub direct: HashMap<KeyChord, Binding>,
    pub normal: HashMap<KeyChord, Action>,
}

fn base_direct() -> HashMap<KeyChord, Binding> {
    use Action::*;
    use KeyToken as T;
    let mut m = HashMap::new();
    let mut d = |chord: KeyChord, action: Action| {
        m.insert(chord, Binding::Do(action));
    };
    d(KeyChord::with_ctrl(T::Char('s')), Save);
    d(KeyChord::with_ctrl(T::Char('q')), Quit);
    d(KeyChord::with_ctrl(T::Char('f')), SearchPrompt);
    d(KeyChord::with_ctrl(T::Char('r')), ReplacePrompt);
    d(KeyChord::with_ctrl(T::Char('z')), Undo);
    d(KeyChord::with_ctrl(T::Char('y')), Redo);
    d(KeyChord::with_ctrl(T::Char('g')), JumpDiagnostic);
    d(KeyChord::with_ctrl(T::Char('d')), SelectNextOccurrence);
    d(KeyChord::with_ctrl(T::Char('p')), CommandPalette);
    d(KeyChord::with_ctrl(T::Char('c')), SystemCopy);
    d(KeyChord::with_ctrl(T::Char('x')), SystemCut);
    d(KeyChord::with_ctrl(T::Char('v')), SystemPaste);
    d(KeyChord::with_ctrl(T::Char(' ')), TriggerCompletion);
    d(KeyChord::with_ctrl(T::Char('o')), OpenFilePrompt);
    d(KeyChord::with_ctrl(T::Char('w')), CloseBuffer);
    d(KeyChord::with_ctrl(T::Char('l')), ToggleWrap);
    d(KeyChord::with_ctrl(T::Char('e')), TogglePreview);
    d(KeyChord::with_ctrl(T::Char('k')), ToggleComment);
    d(KeyChord::with_ctrl(T::Char('t')), FindFilePrompt);
    // Ctrl+Shift+F sería lo esperable, pero una terminal en modo tradicional
    // la manda igual que Ctrl+F. Ctrl+N estaba libre en los tres perfiles.
    d(KeyChord::with_ctrl(T::Char('n')), SearchProjectPrompt);
    d(KeyChord::with_ctrl(T::Char('u')), MacroRecord);
    d(KeyChord::with_ctrl(T::Char('b')), MacroPlay);
    // Ctrl+] es el atajo clásico de "ir a la definición"; la terminal lo
    // manda como Ctrl+5, que es lo que hay que atar para que funcione.
    d(KeyChord::with_ctrl(T::Char('5')), GotoDefinition);
    d(KeyChord::plain(T::F(12)), GotoDefinition);
    d(KeyChord::plain(T::F(6)), RenamePrompt);
    d(KeyChord::with_ctrl(T::PageDown), NextBuffer);
    d(KeyChord::with_ctrl(T::PageUp), PrevBuffer);
    d(KeyChord::plain(T::Enter), InsertNewline);
    d(KeyChord::plain(T::Tab), InsertTab);
    d(KeyChord::with_shift(T::Tab), Unindent);
    d(KeyChord::plain(T::Backspace), Action::Backspace);
    d(KeyChord::plain(T::Delete), DeleteForward);
    d(KeyChord::plain(T::Left), MoveLeft(false));
    d(KeyChord::with_shift(T::Left), MoveLeft(true));
    d(KeyChord::plain(T::Right), MoveRight(false));
    d(KeyChord::with_shift(T::Right), MoveRight(true));
    d(KeyChord::plain(T::Up), MoveUp(false));
    d(KeyChord::with_shift(T::Up), MoveUp(true));
    d(KeyChord::plain(T::Down), MoveDown(false));
    d(KeyChord::with_shift(T::Down), MoveDown(true));
    d(KeyChord::plain(T::Home), MoveHome(false));
    d(KeyChord::with_shift(T::Home), MoveHome(true));
    d(KeyChord::plain(T::End), MoveEnd(false));
    d(KeyChord::with_shift(T::End), MoveEnd(true));
    d(KeyChord::plain(T::PageUp), Action::PageUp(false));
    d(KeyChord::with_shift(T::PageUp), Action::PageUp(true));
    d(KeyChord::plain(T::PageDown), Action::PageDown(false));
    d(KeyChord::with_shift(T::PageDown), Action::PageDown(true));
    m
}

fn base_normal() -> HashMap<KeyChord, Action> {
    use Action::*;
    use KeyToken::*;
    use self::InsertAt as At;
    let mut m = HashMap::new();
    m.insert(KeyChord::plain(Left), MoveLeft(false));
    m.insert(KeyChord::with_shift(Left), MoveLeft(true));
    m.insert(KeyChord::plain(Right), MoveRight(false));
    m.insert(KeyChord::with_shift(Right), MoveRight(true));
    m.insert(KeyChord::plain(Up), MoveUp(false));
    m.insert(KeyChord::with_shift(Up), MoveUp(true));
    m.insert(KeyChord::plain(Down), MoveDown(false));
    m.insert(KeyChord::with_shift(Down), MoveDown(true));
    m.insert(KeyChord::plain(Char('h')), MoveLeft(false));
    m.insert(KeyChord::plain(Char('l')), MoveRight(false));
    m.insert(KeyChord::plain(Char('k')), MoveUp(false));
    m.insert(KeyChord::plain(Char('j')), MoveDown(false));
    m.insert(KeyChord::plain(Char('w')), SelectWord);
    m.insert(KeyChord::plain(Char('x')), SelectLine);
    m.insert(KeyChord::plain(Char('n')), ExpandSelection);
    m.insert(KeyChord::plain(Char('m')), JumpMatchingBracket);
    m.insert(KeyChord::plain(Char('D')), GotoDefinition);
    m.insert(KeyChord::plain(Char('r')), RenamePrompt);
    m.insert(KeyChord::plain(Char('d')), NormalDelete);
    m.insert(KeyChord::plain(Char('c')), NormalChange);
    m.insert(KeyChord::plain(Char('y')), NormalYank);
    m.insert(KeyChord::plain(Char('p')), NormalPaste);
    m.insert(KeyChord::plain(Char('i')), Action::InsertAt(At::SelectionStart));
    m.insert(KeyChord::plain(Char('a')), Action::InsertAt(At::SelectionEnd));
    m.insert(KeyChord::plain(Char('I')), Action::InsertAt(At::LineStart));
    m.insert(KeyChord::plain(Char('A')), Action::InsertAt(At::LineEnd));
    m.insert(KeyChord::plain(Char('o')), OpenBelow);
    m.insert(KeyChord::plain(Char('O')), OpenAbove);
    m.insert(KeyChord::plain(Char('u')), Undo);
    m.insert(KeyChord::plain(Char('U')), Redo);
    m.insert(KeyChord::plain(Esc), EscapeKey);
    m
}

/// El perfil propio de Flint — selección→acción estilo Kakoune/Helix. Es el
/// que arranca por defecto.
pub fn flint_profile() -> Keymap {
    Keymap {
        name: "flint",
        label: "Flint (por defecto)",
        direct: base_direct(),
        normal: base_normal(),
    }
}

/// Sabor Vim: el gesto más reconocible es `u` / `Ctrl+R` para deshacer y
/// rehacer, y `x` borra el carácter bajo el cursor en vez de seleccionar la
/// línea. No es una emulación completa de Vim — no hay secuencias tipo `dd`
/// o `ciw`, porque el modelo de Flint sigue siendo selección→acción, no
/// verbo+movimiento — pero las teclas que sí coinciden, coinciden de verdad.
pub fn vim_profile() -> Keymap {
    let mut normal = base_normal();
    normal.insert(KeyChord::with_ctrl(KeyToken::Char('r')), Action::Redo);
    normal.insert(KeyChord::plain(KeyToken::Char('x')), Action::NormalDelete);
    Keymap {
        name: "vim",
        label: "Vim (sabor)",
        direct: base_direct(),
        normal,
    }
}

/// Sabor Emacs: `Ctrl+X Ctrl+S` guarda y `Ctrl+X Ctrl+C` sale (el prefijo de
/// dos teclas más reconocible de Emacs), `Ctrl+S` busca, `Ctrl+G` cancela —
/// los cuatro reflejos que más se extrañan viniendo de Emacs. La capa modal
/// (NORMAL/INSERT) es la misma que en el perfil Flint: Emacs no es un editor
/// modal, así que no había una convención propia que imitar ahí.
pub fn emacs_profile() -> Keymap {
    let mut direct = base_direct();
    direct.remove(&KeyChord::with_ctrl(KeyToken::Char('s')));
    direct.insert(
        KeyChord::with_ctrl(KeyToken::Char('s')),
        Binding::Do(Action::SearchPrompt),
    );
    direct.remove(&KeyChord::with_ctrl(KeyToken::Char('g')));
    direct.insert(
        KeyChord::with_ctrl(KeyToken::Char('g')),
        Binding::Do(Action::EscapeKey),
    );

    let mut prefix_x = HashMap::new();
    prefix_x.insert(KeyChord::with_ctrl(KeyToken::Char('s')), Action::Save);
    prefix_x.insert(KeyChord::with_ctrl(KeyToken::Char('c')), Action::Quit);
    direct.insert(
        KeyChord::with_ctrl(KeyToken::Char('x')),
        Binding::Prefix(prefix_x),
    );

    Keymap {
        name: "emacs",
        label: "Emacs (sabor)",
        direct,
        normal: base_normal(),
    }
}

pub fn profile_by_name(name: &str) -> Option<Keymap> {
    match name.to_ascii_lowercase().as_str() {
        "flint" | "default" => Some(flint_profile()),
        "vim" => Some(vim_profile()),
        "emacs" => Some(emacs_profile()),
        _ => None,
    }
}

pub fn all_profile_names() -> &'static [&'static str] {
    &["flint", "vim", "emacs"]
}

/// La etiqueta legible de un perfil, sin tener que construirlo entero.
pub fn profile_label(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "vim" => "Vim (sabor)",
        "emacs" => "Emacs (sabor)",
        _ => "Flint (por defecto)",
    }
}

/// Nombre estable de cada acción, y la acción de cada nombre. Es la misma
/// tabla leída en los dos sentidos: un nombre nuevo se agrega una sola vez y
/// queda disponible tanto para `[keys]` en la configuración como para
/// cualquier cosa que necesite decir "esta acción" por escrito.
///
/// Los nombres son en inglés como el resto de los identificadores del
/// código, y siguen el mismo estilo que las claves de `theme.toml`.
fn action_names() -> &'static [(&'static str, Action)] {
    use Action::*;
    use self::InsertAt as At;
    &[
        ("save", Save),
        ("quit", Quit),
        ("search", SearchPrompt),
        ("replace", ReplacePrompt),
        ("search_regex", SearchRegexPrompt),
        ("replace_regex", ReplaceRegexPrompt),
        ("copy", SystemCopy),
        ("cut", SystemCut),
        ("paste", SystemPaste),
        ("undo", Undo),
        ("redo", Redo),
        ("next_diagnostic", JumpDiagnostic),
        ("complete", TriggerCompletion),
        ("toggle_modal", ToggleModalLayer),
        ("select_next_occurrence", SelectNextOccurrence),
        ("command_palette", CommandPalette),
        ("open_file", OpenFilePrompt),
        ("next_buffer", NextBuffer),
        ("prev_buffer", PrevBuffer),
        ("close_buffer", CloseBuffer),
        ("toggle_wrap", ToggleWrap),
        ("toggle_preview", TogglePreview),
        ("move_left", MoveLeft(false)),
        ("select_left", MoveLeft(true)),
        ("move_right", MoveRight(false)),
        ("select_right", MoveRight(true)),
        ("move_up", MoveUp(false)),
        ("select_up", MoveUp(true)),
        ("move_down", MoveDown(false)),
        ("select_down", MoveDown(true)),
        ("move_home", MoveHome(false)),
        ("select_home", MoveHome(true)),
        ("move_end", MoveEnd(false)),
        ("select_end", MoveEnd(true)),
        ("page_up", Action::PageUp(false)),
        ("select_page_up", Action::PageUp(true)),
        ("page_down", Action::PageDown(false)),
        ("select_page_down", Action::PageDown(true)),
        ("newline", InsertNewline),
        ("backspace", Action::Backspace),
        ("delete", DeleteForward),
        ("indent", InsertTab),
        ("unindent", Unindent),
        ("select_word", SelectWord),
        ("select_line", SelectLine),
        ("expand_selection", ExpandSelection),
        ("modal_delete", NormalDelete),
        ("modal_change", NormalChange),
        ("modal_yank", NormalYank),
        ("modal_paste", NormalPaste),
        ("insert", Action::InsertAt(At::SelectionStart)),
        ("append", Action::InsertAt(At::SelectionEnd)),
        ("insert_line_start", Action::InsertAt(At::LineStart)),
        ("append_line_end", Action::InsertAt(At::LineEnd)),
        ("open_below", OpenBelow),
        ("open_above", OpenAbove),
        ("escape", EscapeKey),
        ("toggle_comment", ToggleComment),
        ("jump_matching_bracket", JumpMatchingBracket),
        ("find_file", FindFilePrompt),
        ("search_project", SearchProjectPrompt),
        ("goto_line", GotoLinePrompt),
        ("goto_definition", GotoDefinition),
        ("rename", RenamePrompt),
        ("record_macro", MacroRecord),
        ("play_macro", MacroPlay),
    ]
}

impl Action {
    /// La acción que se llama `name`, o `None` si ese nombre no existe.
    pub fn from_name(name: &str) -> Option<Action> {
        let name = name.trim().to_ascii_lowercase();
        action_names()
            .iter()
            .find(|(n, _)| *n == name)
            .map(|&(_, a)| a)
    }

    /// El nombre estable de esta acción — el mismo que se escribe en la
    /// configuración.
    pub fn name(self) -> &'static str {
        action_names()
            .iter()
            .find(|(_, a)| *a == self)
            .map(|&(n, _)| n)
            .unwrap_or("?")
    }
}

/// Todos los nombres de acción, ordenados — para poder listarlos en un aviso
/// cuando alguien escribe uno que no existe.
pub fn all_action_names() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = action_names().iter().map(|&(n, _)| n).collect();
    v.sort_unstable();
    v
}

/// Interpreta una combinación escrita a mano ("ctrl+s", "shift+tab", "F"),
/// como viene de la configuración. Devuelve el porqué cuando no se entiende,
/// para poder mostrarlo en vez de ignorar la línea en silencio.
///
/// Shift sobre una letra no se guarda como modificador: la mayúscula ya está
/// en el carácter, igual que como llega de la terminal (ver `chord_from_event`),
/// así que "shift+a" y "A" son la misma tecla.
pub fn parse_chord(s: &str) -> Result<KeyChord, String> {
    let mut ctrl = false;
    let mut shift = false;
    let partes: Vec<&str> = s.split('+').map(str::trim).collect();
    // Un "+" suelto ("ctrl++") deja una parte vacía en el medio y la tecla
    // real es el propio "+": se rearma en vez de rechazarlo.
    let (mods, tecla) = match partes.split_last() {
        Some((last, mods)) if !last.is_empty() => (mods, (*last).to_string()),
        Some((_, mods)) => (mods, "+".to_string()),
        None => return Err(format!("\"{s}\" está vacío")),
    };

    for m in mods {
        match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "c" => ctrl = true,
            "shift" | "s" => shift = true,
            "alt" | "meta" | "super" => {
                return Err(format!(
                    "\"{s}\": Flint todavía no distingue el modificador \"{m}\" en el teclado"
                ));
            }
            otro => return Err(format!("\"{s}\": modificador desconocido \"{otro}\"")),
        }
    }

    let token = match tecla.to_ascii_lowercase().as_str() {
        "space" | "espacio" => KeyToken::Char(' '),
        "tab" => KeyToken::Tab,
        "enter" | "return" | "intro" => KeyToken::Enter,
        "esc" | "escape" => KeyToken::Esc,
        "backspace" => KeyToken::Backspace,
        "delete" | "del" | "supr" => KeyToken::Delete,
        "home" | "inicio" => KeyToken::Home,
        "end" | "fin" => KeyToken::End,
        "pageup" | "pgup" | "repag" => KeyToken::PageUp,
        "pagedown" | "pgdn" | "avpag" => KeyToken::PageDown,
        "left" | "izquierda" => KeyToken::Left,
        "right" | "derecha" => KeyToken::Right,
        "up" | "arriba" => KeyToken::Up,
        "down" | "abajo" => KeyToken::Down,
        otra => {
            // "f1".."f12". Se prueba antes que el carácter suelto, porque
            // "f" a secas sí es una tecla válida y "f1" no es ninguna letra.
            if let Some(n) = otra
                .strip_prefix('f')
                .and_then(|d| d.parse::<u8>().ok())
                .filter(|n| (1..=12).contains(n))
            {
                if n == 2 {
                    return Err(format!(
                        "\"{s}\": F2 no se puede reasignar, es el interruptor de la capa modal"
                    ));
                }
                KeyToken::F(n)
            } else {
                let mut chars = tecla.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => KeyToken::Char(c),
                    _ => return Err(format!("\"{s}\": no reconozco la tecla \"{tecla}\"")),
                }
            }
        }
    };

    // Una terminal en modo tradicional no puede distinguir Ctrl+] de Ctrl+5:
    // las dos mandan el mismo byte (0x1D), y lo mismo pasa con Ctrl+\,
    // Ctrl+^ y Ctrl+_. Lo que llega del teclado siempre se ve como el dígito
    // (ver `chord_from_event`), así que escribir "ctrl+]" se normaliza a esa
    // forma en vez de quedar atado a una tecla que nunca va a llegar.
    let token = match (ctrl, token) {
        (true, KeyToken::Char(']')) => KeyToken::Char('5'),
        (true, KeyToken::Char('\\')) => KeyToken::Char('4'),
        (true, KeyToken::Char('^')) => KeyToken::Char('6'),
        (true, KeyToken::Char('_')) => KeyToken::Char('7'),
        (_, t) => t,
    };

    // La mayúscula ya distingue la tecla, así que Shift sobre una letra se
    // aplica al carácter y no queda como modificador — si no, "shift+a" y
    // "A" serían dos entradas distintas de la tabla y solo una andaría.
    if let KeyToken::Char(c) = token {
        let c = if shift {
            c.to_uppercase().next().unwrap_or(c)
        } else {
            c
        };
        return Ok(KeyChord {
            token: KeyToken::Char(c),
            ctrl,
            shift: false,
        });
    }
    Ok(KeyChord { token, ctrl, shift })
}

/// Reemplaza (o borra, con `None`) lo que cuelga de `chord` en la capa
/// directa. Un remapeo pisa un prefijo entero si había uno.
pub fn rebind_direct(map: &mut Keymap, chord: KeyChord, action: Option<Action>) {
    match action {
        Some(a) => {
            map.direct.insert(chord, Binding::Do(a));
        }
        None => {
            map.direct.remove(&chord);
        }
    }
}

/// Lo mismo para la capa modal NORMAL.
pub fn rebind_normal(map: &mut Keymap, chord: KeyChord, action: Option<Action>) {
    match action {
        Some(a) => {
            map.normal.insert(chord, a);
        }
        None => {
            map.normal.remove(&chord);
        }
    }
}

#[cfg(test)]
mod tests_teclas_de_funcion {
    use super::*;

    #[test]
    fn las_teclas_de_funcion_se_escriben_y_se_reconocen() {
        assert_eq!(parse_chord("f5"), Ok(KeyChord::plain(KeyToken::F(5))));
        assert_eq!(parse_chord("F12"), Ok(KeyChord::plain(KeyToken::F(12))));
        assert_eq!(
            parse_chord("ctrl+f5"),
            Ok(KeyChord::with_ctrl(KeyToken::F(5)))
        );
        // "f" a secas sigue siendo la letra, no una tecla de función rota.
        assert_eq!(parse_chord("f"), Ok(KeyChord::plain(KeyToken::Char('f'))));
        assert!(parse_chord("f13").is_err());
        assert!(parse_chord("f0").is_err());
    }

    #[test]
    fn f2_no_se_puede_reasignar() {
        // Es el interruptor de la capa modal: si alguien la ata a otra cosa
        // se queda sin forma de salir del modo.
        let e = parse_chord("f2").unwrap_err();
        assert!(e.contains("capa modal"), "{e}");
    }

    #[test]
    fn la_tecla_de_funcion_llega_del_teclado() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let ev = KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE);
        assert_eq!(chord_from_event(&ev), Some(KeyChord::plain(KeyToken::F(5))));
    }
}

#[cfg(test)]
mod tests_ctrl_corchete {
    use super::*;

    #[test]
    fn ctrl_corchete_se_normaliza_a_lo_que_manda_la_terminal() {
        // Las dos formas tienen que dar la misma tecla, porque la terminal
        // manda el mismo byte para las dos. Atar "ctrl+]" y que no pase nada
        // es peor que no poder atarlo.
        let esperado = KeyChord::with_ctrl(KeyToken::Char('5'));
        assert_eq!(parse_chord("ctrl+]"), Ok(esperado));
        assert_eq!(parse_chord("ctrl+5"), Ok(esperado));
        assert_eq!(
            parse_chord("ctrl+\\"),
            Ok(KeyChord::with_ctrl(KeyToken::Char('4')))
        );
        // Sin Ctrl no se toca: "]" a secas es el corchete.
        assert_eq!(parse_chord("]"), Ok(KeyChord::plain(KeyToken::Char(']'))));
    }

    #[test]
    fn ir_a_la_definicion_esta_atado_a_lo_que_llega_del_teclado() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        // El byte 0x1D que manda Ctrl+] llega como Ctrl+5.
        let ev = KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL);
        let chord = chord_from_event(&ev).expect("es una tecla");
        let km = flint_profile();
        assert!(
            matches!(km.direct.get(&chord), Some(Binding::Do(Action::GotoDefinition))),
            "Ctrl+] tiene que llegar a ir-a-la-definición"
        );
    }
}
