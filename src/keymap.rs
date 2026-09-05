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
