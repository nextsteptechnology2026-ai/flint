mod editor;
mod highlight;
mod keymap;
mod lsp;
mod plugins;
mod theme;
mod ui;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal};
use regex::Regex;
use serde_json::Value;

use editor::{Editor, Layer, Mode, PromptKind};
use keymap::{Action, KeyChord};

const LSP_DEBOUNCE: Duration = Duration::from_millis(500);
/// Recalcular el resaltado de sintaxis vuelve a recorrer el archivo entero
/// (`O(n)`, ver README) — en un archivo grande, hacerlo en cada tecla durante
/// una tanda de tipeo rápido es trabajo desperdiciado, porque solo el último
/// resultado importa. Con este margen de espera, una ráfaga de teclas hace
/// un solo recálculo al final en vez de uno por letra; al abrir un archivo
/// (sin ediciones todavía) el primer resaltado sigue siendo inmediato.
const HIGHLIGHT_DEBOUNCE: Duration = Duration::from_millis(120);
/// Cada cuánto se revisa si `theme.toml` cambió en disco, para recargarlo en
/// caliente — un segundo es seguido para sentirse "en vivo" sin convertir
/// cada vuelta del loop principal en un `stat()`.
const THEME_RELOAD_INTERVAL: Duration = Duration::from_secs(1);

/// Qué hace un renglón de la paleta de comandos al ejecutarse.
#[derive(Clone, Copy)]
enum CommandKind {
    Builtin(Action),
    SwitchProfile(&'static str),
    Plugin(usize),
}

struct PaletteEntry {
    label: String,
    kind: CommandKind,
}

/// Todo el estado propio de un archivo abierto: el editor, su resaltado de
/// sintaxis y su conexión (si tiene) con el documento correspondiente en el
/// servidor de lenguaje compartido. Varios `Buffer` del mismo lenguaje pueden
/// compartir un único `LspClient` en `App` — cada uno abre su propio
/// documento (`doc_uri`) contra el mismo proceso de servidor.
struct Buffer {
    ed: Editor,
    lang: Option<highlight::Lang>,
    highlighter: Option<highlight::LanguageHighlighter>,
    doc_uri: Option<String>,
    lsp_lang_id: Option<&'static str>,
    lsp_synced_version: u64,
}

impl Buffer {
    fn open(path: Option<PathBuf>) -> io::Result<Buffer> {
        let ed = Editor::open(path)?;
        let lang = ed.filename.as_ref().and_then(|p| highlight::lang_for_path(p));
        let highlighter = lang.as_ref().and_then(highlight::LanguageHighlighter::new);
        Ok(Buffer {
            ed,
            lang,
            highlighter,
            doc_uri: None,
            lsp_lang_id: None,
            lsp_synced_version: 0,
        })
    }

    /// Lo que se muestra en la barra de pestañas: solo el nombre de archivo,
    /// no la ruta completa (que ya se ve en el título para el buffer activo).
    fn tab_label(&self) -> String {
        let name = self
            .ed
            .filename
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "[Sin nombre]".to_string());
        if self.ed.dirty {
            format!("{name} •")
        } else {
            name
        }
    }
}

struct App {
    buffers: Vec<Buffer>,
    active: usize,
    text_area: Rect,
    /// `None` cuando hay un solo buffer abierto (no se dibuja barra de
    /// pestañas, así que no hay nada que clickear ahí).
    tabs_area: Option<Rect>,
    lsp: Option<lsp::LspClient>,
    /// El lenguaje que sirve `lsp`, si hay uno corriendo — para saber si un
    /// buffer nuevo del mismo lenguaje puede sumarse al mismo servidor en vez
    /// de necesitar uno propio.
    lsp_lang: Option<&'static str>,
    /// Si `lsp` anunció soporte de sincronización incremental
    /// (`textDocumentSync.change == 2`) al iniciarse — si no, siempre se
    /// manda el documento completo, que es lo único universalmente soportado.
    lsp_incremental: bool,
    /// true justo después de que la última tecla en modo NORMAL fue `x`, para
    /// que una `x` repetida extienda la selección de línea en vez de reiniciarla.
    last_was_select_line: bool,
    keymap: keymap::Keymap,
    /// Cuando la última tecla fue el primer paso de una secuencia con prefijo
    /// (como el `Ctrl+X` de Emacs), esto guarda a qué se puede llegar con la
    /// tecla siguiente.
    pending_prefix: Option<HashMap<KeyChord, Action>>,
    palette_entries: Vec<PaletteEntry>,
    plugin_commands: Vec<plugins::PluginCommand>,
    plugins: Option<plugins::PluginBridge>,
    theme: theme::Theme,
    /// Ruta del `theme.toml` en uso, si vino de un archivo real (no la
    /// paleta de fábrica) — para poder recargarlo en caliente.
    theme_path: Option<PathBuf>,
    theme_mtime: Option<std::time::SystemTime>,
    /// Cuándo toca volver a mirar la fecha de modificación del archivo de
    /// tema — revisarlo en cada vuelta del loop sería un `stat()` de más
    /// por cada tecla; una vez por segundo alcanza para sentirse "en vivo".
    theme_next_check: Instant,
    /// `None` si no se pudo abrir el portapapeles del sistema al arrancar
    /// (sin servidor gráfico, por ejemplo) — ahí copiar/cortar/pegar caen
    /// solo al registro interno, sin romper nada.
    clipboard: Option<arboard::Clipboard>,
}

fn lang_id_str(lang: &highlight::Lang) -> &'static str {
    match lang {
        highlight::Lang::Rust => "rust",
        highlight::Lang::Python => "python",
        highlight::Lang::Json => "json",
        highlight::Lang::Toml => "toml",
    }
}

/// Comandos fijos de la paleta — lo que Flint ya sabe hacer, además de todo
/// lo que un plugin agregue en tiempo de carga.
fn builtin_palette_entries() -> Vec<PaletteEntry> {
    let items: &[(&str, Action)] = &[
        ("Guardar", Action::Save),
        ("Salir", Action::Quit),
        ("Buscar…", Action::SearchPrompt),
        ("Reemplazar…", Action::ReplacePrompt),
        ("Buscar (regex)…", Action::SearchRegexPrompt),
        ("Reemplazar (regex)…", Action::ReplaceRegexPrompt),
        ("Deshacer", Action::Undo),
        ("Rehacer", Action::Redo),
        ("Saltar al siguiente diagnóstico", Action::JumpDiagnostic),
        ("Autocompletar (LSP)", Action::TriggerCompletion),
        ("Alternar capa modal (F2)", Action::ToggleModalLayer),
        (
            "Seleccionar siguiente aparición (multi-cursor)",
            Action::SelectNextOccurrence,
        ),
        ("Abrir archivo…", Action::OpenFilePrompt),
        ("Siguiente buffer", Action::NextBuffer),
        ("Buffer anterior", Action::PrevBuffer),
        ("Cerrar buffer", Action::CloseBuffer),
        ("Alternar ajuste de línea", Action::ToggleWrap),
        ("Indentar líneas (Tab)", Action::InsertTab),
        ("Des-indentar líneas (Shift+Tab)", Action::Unindent),
    ];
    let mut entries: Vec<PaletteEntry> = items
        .iter()
        .map(|&(label, action)| PaletteEntry {
            label: label.to_string(),
            kind: CommandKind::Builtin(action),
        })
        .collect();
    for name in keymap::all_profile_names() {
        entries.push(PaletteEntry {
            label: format!("Perfil de teclado: {}", keymap::profile_label(name)),
            kind: CommandKind::SwitchProfile(name),
        });
    }
    entries
}

const HELP_TEXT: &str = concat!(
    "Flint ",
    env!("CARGO_PKG_VERSION"),
    " — editor de terminal: capa modal opcional, LSP, multi-cursor, temas y plugins en Lua.\n",
    "\n",
    "USO:\n",
    "    flint [opciones] [archivo]\n",
    "\n",
    "OPCIONES:\n",
    "    --profile <flint|vim|emacs>   Perfil de atajos de teclado (por defecto: flint)\n",
    "    --theme <ruta>                 Archivo de tema .toml (por defecto: ~/.config/flint/theme.toml si existe)\n",
    "    -h, --help                     Muestra esta ayuda y sale\n",
    "    -V, --version                  Muestra la versión y sale\n",
    "\n",
    "ATAJOS PRINCIPALES (modo directo, perfil por defecto):\n",
    "    Ctrl+S  Guardar         Ctrl+Q  Salir            Ctrl+F  Buscar\n",
    "    Ctrl+R  Reemplazar      Ctrl+Z  Deshacer          Ctrl+Y  Rehacer\n",
    "    Ctrl+P  Paleta de comandos      Ctrl+Espacio  Autocompletar (LSP)\n",
    "    Ctrl+G  Siguiente diagnóstico   Ctrl+D  +cursor en la siguiente aparición\n",
    "    Ctrl+C/X/V  Copiar/Cortar/Pegar (portapapeles del sistema)\n",
    "    Ctrl+L  Alternar ajuste de línea\n",
    "    F2      Activar/desactivar la capa modal (NORMAL/INSERT)\n",
    "    Alt+clic  Agregar un cursor donde se hace clic\n",
    "\n",
    "BUFFERS (varios archivos a la vez):\n",
    "    Ctrl+O  Abrir archivo (en un buffer nuevo)     Ctrl+W  Cerrar buffer actual\n",
    "    Ctrl+PageDown/PageUp  Siguiente/anterior buffer\n",
    "\n",
    "CAPA MODAL — NORMAL (tras F2):\n",
    "    w/x   Seleccionar palabra/línea   n     Expandir selección (sintaxis)\n",
    "    d/c   Borrar/Cambiar              y/p   Copiar/Pegar\n",
    "    i/a/I/A   Insertar (selección/línea)     o/O   Abrir línea abajo/arriba\n",
    "    u/U   Deshacer/Rehacer            Esc   Deseleccionar / volver\n",
    "\n",
    "La paleta de comandos (Ctrl+P) también tiene \"Buscar (regex)…\" y\n",
    "\"Reemplazar (regex)…\" — mismo flujo, pero con expresiones regulares.\n",
    "\n",
    "Manual completo (temas, plugins, perfiles, todos los atajos): ver MANUAL.md\n",
    "en el repositorio del proyecto.\n",
);

fn parse_args() -> (Option<PathBuf>, String, Option<PathBuf>) {
    let mut path = None;
    let mut profile = "flint".to_string();
    let mut theme_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--profile" {
            if let Some(v) = args.next() {
                profile = v;
            }
        } else if arg == "--theme" {
            if let Some(v) = args.next() {
                theme_path = Some(PathBuf::from(v));
            }
        } else if arg == "-h" || arg == "--help" {
            print!("{HELP_TEXT}");
            std::process::exit(0);
        } else if arg == "-V" || arg == "--version" {
            println!("flint {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        } else {
            path = Some(PathBuf::from(arg));
        }
    }
    (path, profile, theme_path)
}

fn main() -> io::Result<()> {
    let (path, profile_name, theme_arg) = parse_args();
    let mut buffer = Buffer::open(path)?;

    let keymap = keymap::profile_by_name(&profile_name).unwrap_or_else(|| {
        buffer.ed.status = format!(
            "{} — perfil de teclado desconocido \"{profile_name}\", uso Flint",
            buffer.ed.status
        );
        keymap::flint_profile()
    });
    if keymap.name != "flint" {
        buffer.ed.status = format!("{} — perfil de teclado: {}", buffer.ed.status, keymap.name);
    }

    // `--theme <ruta>` explícito manda; si no, `~/.config/flint/theme.toml`
    // si existe; si no hay ninguno, la paleta ámbar de siempre. Si el archivo
    // sí existe, se recuerda su ruta y fecha de modificación para recargarlo
    // en caliente más adelante (ver `check_theme_reload`) — editar
    // `theme.toml` y guardar aplica los cambios sin reabrir Flint.
    let theme_path = theme_arg.or_else(theme::Theme::default_path);
    let mut watched_theme_path: Option<PathBuf> = None;
    let mut theme_mtime: Option<std::time::SystemTime> = None;
    let theme = match theme_path {
        Some(p) if p.exists() => {
            let (t, warnings) = theme::Theme::load(&p);
            if let Some(first) = warnings.first() {
                buffer.ed.status = format!("{} — tema: {first}", buffer.ed.status);
            } else {
                buffer.ed.status = format!("{} — tema: {}", buffer.ed.status, p.display());
            }
            theme_mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            watched_theme_path = Some(p);
            t
        }
        _ => theme::Theme::default(),
    };
    buffer.ed.tab_width = theme.tab_width;

    if let Some(l) = &buffer.lang {
        buffer.ed.status = format!("{} — resaltado: {}", buffer.ed.status, l.label());
    }

    let plugins_dir = PathBuf::from("plugins");
    let (plugin_host, plugin_commands, plugin_errors) = plugins::PluginBridge::load(&plugins_dir);
    let mut palette_entries = builtin_palette_entries();
    for (i, cmd) in plugin_commands.iter().enumerate() {
        palette_entries.push(PaletteEntry {
            label: format!("{} (plugin)", cmd.label),
            kind: CommandKind::Plugin(i),
        });
    }
    if !plugin_commands.is_empty() {
        buffer.ed.status = format!(
            "{} · {} comando(s) de plugin",
            buffer.ed.status,
            plugin_commands.len()
        );
    }
    if let Some(first_error) = plugin_errors.first() {
        buffer.ed.status = format!("{} · error de plugin: {first_error}", buffer.ed.status);
    }

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        theme.crossterm_cursor_style()
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let lang = buffer.lang;
    let (lsp_client, doc_uri, lsp_lang_id, lsp_incremental) =
        setup_lsp(&mut terminal, &mut buffer.ed, lang.as_ref(), &theme);
    buffer.doc_uri = doc_uri;
    buffer.lsp_lang_id = lsp_lang_id;

    let mut app = App {
        buffers: vec![buffer],
        active: 0,
        text_area: Rect::default(),
        tabs_area: None,
        lsp: lsp_client,
        lsp_lang: lsp_lang_id,
        lsp_incremental,
        last_was_select_line: false,
        keymap,
        pending_prefix: None,
        palette_entries,
        plugin_commands,
        plugins: Some(plugin_host),
        theme,
        theme_path: watched_theme_path,
        theme_mtime,
        theme_next_check: Instant::now() + THEME_RELOAD_INTERVAL,
        clipboard: arboard::Clipboard::new().ok(),
    };

    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::SetCursorStyle::DefaultUserShape
    )?;
    terminal.show_cursor()?;

    result
}

/// Prepara el cliente LSP para el lenguaje detectado, si Flint sabe de un
/// servidor para él. Si el servidor no está instalado, ofrece instalarlo
/// (nunca en silencio: siempre se pregunta primero).
/// El último `bool` dice si el servidor soporta sincronización incremental
/// — `false` (incluso sin LSP) es siempre seguro, porque hace que
/// `sync_lsp_if_needed` mande el documento completo, como antes.
fn setup_lsp(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ed: &mut Editor,
    lang: Option<&highlight::Lang>,
    theme: &theme::Theme,
) -> (Option<lsp::LspClient>, Option<String>, Option<&'static str>, bool) {
    let Some(lang) = lang else {
        return (None, None, None, false);
    };
    let Some(cmd) = lang.lsp_command() else {
        return (None, None, None, false);
    };
    let Some(path) = ed.filename.clone() else {
        return (None, None, None, false);
    };
    let lang_id = lang_id_str(lang);
    let uri = lsp::file_uri(&path);
    let root = lsp::file_uri(path.parent().unwrap_or(Path::new(".")));

    let mut client = match lsp::LspClient::spawn(cmd) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !prompt_install(terminal, ed, cmd, theme) {
                ed.status = format!("Sin LSP para este archivo (falta {cmd})");
                return (None, None, None, false);
            }
            match lsp::LspClient::spawn(cmd) {
                Ok(c) => c,
                Err(e2) => {
                    ed.status = format!("Sigue sin encontrarse {cmd}: {e2}");
                    return (None, None, None, false);
                }
            }
        }
        Err(e) => {
            ed.status = format!("No se pudo iniciar {cmd}: {e}");
            return (None, None, None, false);
        }
    };

    ed.status = format!("Iniciando {cmd}…");
    let _ = terminal.draw(|f| {
        ui::draw(f, ed, &[], &[], &[], 0, theme);
    });

    let (ok, incremental) = finish_init(&mut client, ed, &uri, &root, lang_id, cmd);
    if ok {
        (Some(client), Some(uri), Some(lang_id), incremental)
    } else {
        (None, None, None, false)
    }
}

/// `bool` de retorno: si el `initialize` salió bien. El segundo valor dice
/// si el servidor anunció soporte de sincronización incremental
/// (`textDocumentSync.change == 2`) — si no, `sync_lsp_if_needed` manda
/// siempre el documento completo, que es lo único que todo servidor LSP
/// soporta sin excepción.
fn finish_init(
    client: &mut lsp::LspClient,
    ed: &mut Editor,
    uri: &str,
    root: &str,
    lang_id: &'static str,
    cmd: &str,
) -> (bool, bool) {
    let id = match client.initialize(root) {
        Ok(id) => id,
        Err(e) => {
            ed.status = format!("No se pudo hablar con {cmd}: {e}");
            return (false, false);
        }
    };
    let Some(result) = wait_for_response(client, id, Duration::from_secs(20)) else {
        ed.status = format!("{cmd} no respondió a tiempo; sigo sin LSP");
        return (false, false);
    };
    let incremental = supports_incremental_sync(&result);
    let _ = client.send_initialized();
    let _ = client.did_open(uri, lang_id, &ed.rope.to_string());
    let sync_tag = if incremental { " (sync incremental)" } else { " (sync completo)" };
    ed.status = format!("{} · {cmd} listo{sync_tag}", ed.status);
    (true, incremental)
}

/// `textDocumentSync` en la respuesta de `initialize` puede venir como un
/// número (0=None, 1=Full, 2=Incremental) o como un objeto con un campo
/// `change` que es ese mismo número — el spec de LSP permite las dos formas.
fn supports_incremental_sync(result: &Value) -> bool {
    let sync = result.get("capabilities").and_then(|c| c.get("textDocumentSync"));
    match sync {
        Some(Value::Number(n)) => n.as_u64() == Some(2),
        Some(Value::Object(_)) => sync.and_then(|s| s.get("change")).and_then(Value::as_u64) == Some(2),
        _ => false,
    }
}

/// Espera la respuesta a la petición `id`, hasta `timeout`; devuelve su
/// campo `"result"` si llegó y lo tenía.
fn wait_for_response(client: &lsp::LspClient, id: u64, timeout: Duration) -> Option<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return None;
        }
        let slice = (deadline - now).min(Duration::from_millis(300));
        if let Some(v) = client.recv_timeout(slice)
            && v.get("id").and_then(Value::as_u64) == Some(id)
        {
            return v.get("result").cloned();
        }
    }
}

/// Diálogo mínimo, autónomo, que se dibuja antes de que arranque el bucle
/// principal: pregunta si instalar el servidor de lenguaje y, si el usuario
/// confirma, lo instala de verdad (bloqueante, una sola vez).
fn prompt_install(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ed: &mut Editor,
    cmd: &str,
    theme: &theme::Theme,
) -> bool {
    let install_cmd = "rustup";
    let install_args = ["component", "add", "rust-analyzer"];
    ed.status = format!(
        "{cmd} no está instalado. ¿Instalarlo con \"{install_cmd} {}\"? (s = sí, n = no): ",
        install_args.join(" ")
    );
    loop {
        let _ = terminal.draw(|f| {
            ui::draw(f, ed, &[], &[], &[], 0, theme);
        });
        if matches!(event::poll(Duration::from_millis(200)), Ok(true))
            && let Ok(Event::Key(key)) = event::read()
        {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            match key.code {
                KeyCode::Char('s') | KeyCode::Char('S') => break,
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => return false,
                _ => {}
            }
        }
    }

    ed.status = format!("Instalando {cmd}…");
    let _ = terminal.draw(|f| {
        ui::draw(f, ed, &[], &[], &[], 0, theme);
    });
    match std::process::Command::new(install_cmd)
        .args(install_args)
        .status()
    {
        Ok(s) if s.success() => {
            ed.status = format!("{cmd} instalado");
            true
        }
        Ok(s) => {
            ed.status = format!("La instalación de {cmd} falló (código {:?})", s.code());
            false
        }
        Err(e) => {
            ed.status = format!("No se pudo ejecutar {install_cmd}: {e}");
            false
        }
    }
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        {
            let buf = &mut app.buffers[app.active];
            let quiet_enough = buf
                .ed
                .last_edit_instant()
                .is_none_or(|t| t.elapsed() >= HIGHLIGHT_DEBOUNCE);
            if quiet_enough {
                highlight::refresh(&mut buf.ed, buf.highlighter.as_ref());
            }
        }
        poll_lsp(app);
        sync_lsp_if_needed(app);
        check_theme_reload(app);

        let palette_labels: Vec<String> = match &app.buffers[app.active].ed.mode {
            Mode::Palette { query, .. } => filtered_palette_indices(app, query)
                .into_iter()
                .filter_map(|i| app.palette_entries.get(i).map(|e| e.label.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let completion_view: Vec<(String, Option<String>)> = match &app.buffers[app.active].ed.mode {
            Mode::Completion { items, prefix, .. } => filtered_completion_indices(items, prefix)
                .into_iter()
                .map(|i| (items[i].label.clone(), items[i].detail.clone()))
                .collect(),
            _ => Vec::new(),
        };
        let tab_labels: Vec<String> = app.buffers.iter().map(Buffer::tab_label).collect();
        terminal.draw(|f| {
            let areas = ui::draw(
                f,
                &mut app.buffers[app.active].ed,
                &palette_labels,
                &completion_view,
                &tab_labels,
                app.active,
                &app.theme,
            );
            app.text_area = areas.text_area;
            app.tabs_area = areas.tabs_area;
        })?;

        if process_pending_quit(app) {
            return Ok(());
        }

        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    handle_key(app, key, app.text_area.height.max(1) as usize);
                }
                Event::Mouse(m) => handle_mouse(app, m),
                _ => {}
            }
        }

        if process_pending_quit(app) {
            return Ok(());
        }
    }
}

/// `ed.should_quit` (que ya maneja todo el flujo de "¿guardar antes de
/// cerrar?" de `try_quit`/`submit_prompt`, sin cambios) significa acá "cerrar
/// el buffer activo", no "salir del proceso" — con varios buffers abiertos,
/// Ctrl+Q los va cerrando de a uno. El proceso solo termina cuando se cierra
/// el último.
fn process_pending_quit(app: &mut App) -> bool {
    if !app.buffers[app.active].ed.should_quit {
        return false;
    }
    close_active_buffer(app);
    app.buffers.is_empty()
}

fn close_active_buffer(app: &mut App) {
    let buf = &app.buffers[app.active];
    if let (Some(uri), Some(client)) = (buf.doc_uri.clone(), app.lsp.as_mut()) {
        let _ = client.did_close(&uri);
    }
    app.buffers.remove(app.active);
    if app.active >= app.buffers.len() {
        app.active = app.buffers.len().saturating_sub(1);
    }
}

fn poll_lsp(app: &mut App) {
    if app.lsp.is_none() {
        return;
    }
    for _ in 0..64 {
        let Some(client) = app.lsp.as_ref() else { break };
        let Some(msg) = client.try_recv() else { break };
        handle_lsp_message(app, msg);
    }
    if let Some(client) = app.lsp.as_mut()
        && !client.is_alive()
    {
        app.lsp = None;
        app.lsp_lang = None;
        for buf in app.buffers.iter_mut() {
            buf.doc_uri = None;
            buf.lsp_lang_id = None;
        }
        app.buffers[app.active].ed.status = "El servidor de lenguaje se cerró; sigo sin LSP".to_string();
    }
}

fn handle_lsp_message(app: &mut App, msg: Value) {
    // Petición del servidor hacia nosotros: trae "id" y "method" a la vez.
    if let (Some(id), Some(method)) = (
        msg.get("id").cloned(),
        msg.get("method").and_then(Value::as_str).map(str::to_string),
    ) {
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        if let Some(client) = app.lsp.as_mut() {
            client.respond_default(id, &method, &params);
        }
        return;
    }
    // Notificación del servidor: trae "method", sin "id".
    if let Some(method) = msg.get("method").and_then(Value::as_str) {
        if method == "textDocument/publishDiagnostics" {
            apply_diagnostics(app, msg.get("params"));
        }
        return;
    }
    // Respuesta a algo que pedimos nosotros: trae "id", sin "method".
    if let Some(id) = msg.get("id").and_then(Value::as_u64) {
        let pending = app.lsp.as_mut().and_then(|c| c.pending.remove(&id));
        if let Some(lsp::Pending::Completion { buffer, trigger, prefix }) = pending {
            apply_completion(app, buffer, trigger, prefix, msg.get("result"));
        }
    }
}

/// Los diagnósticos llegan con la URI del documento al que corresponden —
/// hay que buscar cuál de los buffers abiertos es, en vez de asumir que es
/// siempre el activo (el servidor puede volver a analizar un archivo que ya
/// no se está mirando).
fn apply_diagnostics(app: &mut App, params: Option<&Value>) {
    let Some(params) = params else { return };
    let Some(uri) = params.get("uri").and_then(Value::as_str) else {
        return;
    };
    let Some(idx) = app.buffers.iter().position(|b| b.doc_uri.as_deref() == Some(uri)) else {
        return;
    };
    let Some(items) = params.get("diagnostics").and_then(Value::as_array) else {
        return;
    };
    let mut diags = Vec::with_capacity(items.len());
    for item in items {
        let range = item.get("range");
        let start = range.and_then(|r| r.get("start"));
        let end = range.and_then(|r| r.get("end"));
        let line = start
            .and_then(|s| s.get("line"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        // El servidor manda columnas en unidades UTF-16 — hay que convertir
        // a caracteres (lo que usa `Diagnostic`/la UI acá adentro) antes de
        // guardarlas, o el subrayado queda corrido en líneas con texto no
        // ASCII antes del rango.
        let start_col_utf16 = start
            .and_then(|s| s.get("character"))
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let end_line = end
            .and_then(|e| e.get("line"))
            .and_then(Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(line);
        let end_col_utf16 = end
            .and_then(|e| e.get("character"))
            .and_then(Value::as_u64)
            .unwrap_or(start_col_utf16 as u64) as usize;
        let start_col = app.buffers[idx].ed.utf16_col_to_char(line, start_col_utf16);
        let end_col = app.buffers[idx].ed.utf16_col_to_char(end_line, end_col_utf16);
        let severity = match item.get("severity").and_then(Value::as_u64) {
            Some(1) => editor::Severity::Error,
            Some(2) => editor::Severity::Warning,
            Some(3) => editor::Severity::Info,
            Some(4) => editor::Severity::Hint,
            _ => editor::Severity::Warning,
        };
        let message = item
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        diags.push(editor::Diagnostic {
            line,
            start_col,
            end_line,
            end_col,
            severity,
            message,
        });
    }
    let count = diags.len();
    app.buffers[idx].ed.diagnostics = diags;
    if count > 0 {
        app.buffers[idx].ed.status = format!("{count} diagnóstico(s) — Ctrl+G para saltar al siguiente");
    } else {
        app.buffers[idx].ed.status = "Sin diagnósticos".to_string();
    }
}

/// `buffer` es el índice que pidió el autocompletado (guardado en
/// `Pending::Completion` al mandar la petición) — puede que ya no sea el
/// buffer activo, o incluso que se haya cerrado mientras se esperaba la
/// respuesta, así que se descarta en silencio si ya no existe. `trigger` es
/// dónde empieza el identificador ya tipeado (no el cursor) — el punto desde
/// el que se borra y reemplaza al aceptar una sugerencia; `prefix` es ese
/// identificador, para que el popup arranque ya filtrado por lo que se
/// había escrito antes de pedir el autocompletado.
fn apply_completion(
    app: &mut App,
    buffer: usize,
    trigger: (usize, usize),
    prefix: String,
    result: Option<&Value>,
) {
    let Some(target) = app.buffers.get_mut(buffer) else {
        return;
    };
    let items_val = result.and_then(|r| {
        r.as_array()
            .cloned()
            .or_else(|| r.get("items").and_then(Value::as_array).cloned())
    });
    let items: Vec<editor::CompletionEntry> = items_val
        .unwrap_or_default()
        .into_iter()
        .filter_map(|it| {
            let label = it.get("label").and_then(Value::as_str)?.to_string();
            let detail = it
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_string);
            // `insertTextFormat` 2 = Snippet (`$0`, `${1:nombre}`...) — Flint
            // no expande snippets, así que en ese caso se ignora `insertText`
            // y se usa `label` en cambio (mismo comportamiento que ya había).
            let is_snippet = it.get("insertTextFormat").and_then(Value::as_u64) == Some(2);
            let insert_text = if is_snippet {
                None
            } else {
                it.get("insertText").and_then(Value::as_str).map(str::to_string)
            };
            Some(editor::CompletionEntry { label, detail, insert_text })
        })
        .take(50)
        .collect();

    if items.is_empty() {
        target.ed.status = "Sin sugerencias".to_string();
        return;
    }
    target.ed.mode = Mode::Completion {
        items,
        selected: 0,
        trigger: editor::Position { line: trigger.0, col: trigger.1 },
        prefix,
    };
}

/// Sincroniza con el servidor TODOS los buffers con un documento abierto que
/// tengan cambios pendientes, no solo el activo — si no, un buffer editado y
/// luego dejado de lado (cambiando a otra pestaña) se quedaría desactualizado
/// en el servidor hasta volver a él.
/// Sincroniza cada buffer con documento abierto que tenga cambios
/// pendientes. Cuando lo que se acumuló desde la última vez es una sola
/// edición por vez (el caso común: tipear, borrar, con un cursor) y el
/// servidor anunció soporte incremental, manda solo esos deltas
/// (`did_change_incremental`) — si no, manda el documento completo
/// (`did_change`), que es lo único que todo servidor soporta.
fn sync_lsp_if_needed(app: &mut App) {
    if app.lsp.is_none() {
        return;
    }
    let incremental_ok = app.lsp_incremental;
    for i in 0..app.buffers.len() {
        let buf = &app.buffers[i];
        let Some(uri) = buf.doc_uri.clone() else { continue };
        if buf.ed.content_version == buf.lsp_synced_version {
            continue;
        }
        let quiet_enough = buf
            .ed
            .last_edit_instant()
            .is_none_or(|t| t.elapsed() >= LSP_DEBOUNCE);
        if !quiet_enough {
            continue;
        }
        let version = buf.ed.content_version;
        let plan = app.buffers[i].ed.take_lsp_sync_plan();
        let ok = match plan {
            editor::LspSyncPlan::None => true,
            editor::LspSyncPlan::Incremental(edits) if incremental_ok => {
                let changes: Vec<lsp::LspContentChange> = edits
                    .iter()
                    .map(|e| lsp::LspContentChange {
                        start: (e.start_line, e.start_col_utf16),
                        end: (e.end_line, e.end_col_utf16),
                        text: e.text.clone(),
                    })
                    .collect();
                app.lsp.as_mut().unwrap().did_change_incremental(&uri, &changes).is_ok()
            }
            editor::LspSyncPlan::Incremental(_) | editor::LspSyncPlan::Full => {
                let text = app.buffers[i].ed.rope.to_string();
                app.lsp.as_mut().unwrap().did_change(&uri, &text).is_ok()
            }
        };
        if ok {
            app.buffers[i].lsp_synced_version = version;
        }
    }
}

/// Recarga `theme.toml` en caliente si cambió en disco desde la última vez
/// que se miró — así se puede editar el tema y ver el resultado en Flint sin
/// reabrirlo. Solo revisa la fecha de modificación (barato); el parseo
/// completo (`Theme::load`, ya usado al arrancar) solo corre cuando de
/// verdad cambió.
fn check_theme_reload(app: &mut App) {
    let Some(path) = app.theme_path.as_ref() else {
        return;
    };
    let now = Instant::now();
    if now < app.theme_next_check {
        return;
    }
    app.theme_next_check = now + THEME_RELOAD_INTERVAL;

    let Ok(mtime) = std::fs::metadata(path).and_then(|m| m.modified()) else {
        return;
    };
    if Some(mtime) == app.theme_mtime {
        return;
    }
    app.theme_mtime = Some(mtime);
    let (theme, warnings) = theme::Theme::load(path);
    app.theme = theme;
    // `tab_width` vive en el tema pero lo consume cada buffer al dibujarse,
    // así que la recarga tiene que empujarlo a todos — si no, un cambio de
    // ancho solo se vería en los buffers abiertos después de recargar.
    for b in &mut app.buffers {
        b.ed.tab_width = app.theme.tab_width;
    }
    app.buffers[app.active].ed.status = match warnings.first() {
        Some(first) => format!("Tema recargado — {first}"),
        None => "Tema recargado".to_string(),
    };
}

fn handle_key(app: &mut App, key: KeyEvent, page_size: usize) {
    match app.buffers[app.active].ed.mode {
        Mode::Editing => handle_layer_key(app, key, page_size),
        Mode::Prompt { .. } => handle_prompt_key(app, key),
        Mode::Completion { .. } => handle_completion_key(app, key),
        Mode::Palette { .. } => handle_palette_key(app, key),
    }
}

/// F2 conmuta la capa modal opcional desde cualquiera de sus dos estados y
/// nunca es remapeable — es el único interruptor global fijo. Todo lo demás
/// pasa por el keymap del perfil activo: primero se resuelve si la tecla es
/// la continuación de una secuencia con prefijo pendiente (`Ctrl+X` de
/// Emacs, por ejemplo); si no, se busca en la tabla de la capa activa
/// (`direct` cubre tanto Direct como INSERT, que tipean igual).
fn handle_layer_key(app: &mut App, key: KeyEvent, page_size: usize) {
    if key.code == KeyCode::F(2) {
        toggle_modal_layer(app);
        return;
    }
    let Some(chord) = keymap::chord_from_event(&key) else {
        return;
    };

    if let Some(prefix_map) = app.pending_prefix.take() {
        if let Some(&action) = prefix_map.get(&chord) {
            dispatch_action(app, action, page_size);
        } else {
            app.buffers[app.active].ed.status = "Secuencia cancelada".to_string();
        }
        return;
    }

    match app.buffers[app.active].ed.layer {
        Layer::Direct | Layer::ModalInsert => match app.keymap.direct.get(&chord) {
            Some(keymap::Binding::Do(action)) => dispatch_action(app, *action, page_size),
            Some(keymap::Binding::Prefix(sub)) => {
                app.pending_prefix = Some(sub.clone());
                app.buffers[app.active].ed.status = "…".to_string();
            }
            None => {
                if let KeyCode::Char(c) = key.code
                    && !chord.ctrl
                {
                    app.buffers[app.active].ed.insert_char(c);
                }
            }
        },
        Layer::ModalNormal => {
            if let Some(&action) = app.keymap.normal.get(&chord) {
                dispatch_action(app, action, page_size);
            } else if chord.ctrl
                && let Some(keymap::Binding::Do(action)) = app.keymap.direct.get(&chord)
            {
                dispatch_action(app, *action, page_size);
            }
        }
    }
}

fn toggle_modal_layer(app: &mut App) {
    match app.buffers[app.active].ed.layer {
        Layer::Direct => {
            app.buffers[app.active].ed.layer = Layer::ModalNormal;
            app.buffers[app.active].ed.selection_anchor = None;
            app.buffers[app.active].ed.status = "Capa modal activada — NORMAL".to_string();
        }
        Layer::ModalNormal | Layer::ModalInsert => {
            app.buffers[app.active].ed.layer = Layer::Direct;
            app.buffers[app.active].ed.selection_anchor = None;
            app.buffers[app.active].ed.status = "Capa modal desactivada — modo directo".to_string();
        }
    }
}

/// Envuelve `execute_action` para llevar la cuenta de "la tecla anterior fue
/// seleccionar línea", que `select_line` necesita para decidir si extender
/// o empezar de nuevo.
fn dispatch_action(app: &mut App, action: Action, page_size: usize) {
    let was_select_line = matches!(action, Action::SelectLine);
    execute_action(app, action, page_size);
    app.last_was_select_line = was_select_line;
}

/// El único lugar que sabe qué hace cada `Action` — un perfil de teclado
/// nuevo, o un remapeo propio, solo necesitan decidir qué tecla dispara cuál;
/// el significado de la acción no cambia.
fn execute_action(app: &mut App, action: Action, page_size: usize) {
    match action {
        Action::Save => try_save(&mut app.buffers[app.active].ed, false),
        Action::Quit => {
            let multi = app.buffers.len() > 1;
            try_quit(&mut app.buffers[app.active].ed, multi);
        }
        Action::SearchPrompt => {
            let prefill = app.buffers[app.active].ed.last_search.clone().unwrap_or_default();
            app.buffers[app.active].ed.mode = Mode::Prompt {
                kind: PromptKind::Search,
                buffer: prefill,
                label: "Buscar: ".to_string(),
            };
        }
        Action::ReplacePrompt => {
            app.buffers[app.active].ed.mode = Mode::Prompt {
                kind: PromptKind::ReplaceSearch,
                buffer: String::new(),
                label: "Reemplazar — buscar: ".to_string(),
            };
        }
        Action::SearchRegexPrompt => {
            let prefill = app.buffers[app.active].ed.last_search.clone().unwrap_or_default();
            app.buffers[app.active].ed.mode = Mode::Prompt {
                kind: PromptKind::SearchRegex,
                buffer: prefill,
                label: "Buscar (regex): ".to_string(),
            };
        }
        Action::ReplaceRegexPrompt => {
            app.buffers[app.active].ed.mode = Mode::Prompt {
                kind: PromptKind::ReplaceRegexSearch,
                buffer: String::new(),
                label: "Reemplazar (regex) — buscar: ".to_string(),
            };
        }
        Action::SystemCopy => {
            match app.buffers[app.active].ed.selected_text() {
                Some(text) => {
                    set_clipboard(app, text);
                    app.buffers[app.active].ed.status = "Copiado".to_string();
                }
                None => app.buffers[app.active].ed.status = "Nada seleccionado".to_string(),
            }
        }
        Action::SystemCut => match app.buffers[app.active].ed.selected_text() {
            Some(text) => {
                set_clipboard(app, text);
                app.buffers[app.active].ed.delete_selection_action();
                app.buffers[app.active].ed.status = "Cortado".to_string();
            }
            None => app.buffers[app.active].ed.status = "Nada seleccionado".to_string(),
        },
        Action::SystemPaste => paste_from_clipboard(app),
        Action::Undo => {
            app.buffers[app.active].ed.status = if app.buffers[app.active].ed.undo() {
                "Deshecho".to_string()
            } else {
                "Nada que deshacer".to_string()
            };
        }
        Action::Redo => {
            app.buffers[app.active].ed.status = if app.buffers[app.active].ed.redo() {
                "Rehecho".to_string()
            } else {
                "Nada que rehacer".to_string()
            };
        }
        Action::JumpDiagnostic => {
            app.buffers[app.active].ed.status = app.buffers[app.active]
                .ed
                .jump_to_next_diagnostic()
                .unwrap_or_else(|| "Sin diagnósticos".to_string());
        }
        Action::TriggerCompletion => trigger_completion(app),
        Action::ToggleModalLayer => toggle_modal_layer(app),
        Action::SelectNextOccurrence => {
            app.buffers[app.active].ed.status = if app.buffers[app.active].ed.select_next_occurrence() {
                format!("{} cursores", 1 + app.buffers[app.active].ed.secondary.len())
            } else {
                "Nada que seleccionar — primero selecciona texto".to_string()
            };
        }
        Action::CommandPalette => open_command_palette(app),
        Action::MoveLeft(extend) => app.buffers[app.active].ed.move_left(extend),
        Action::MoveRight(extend) => app.buffers[app.active].ed.move_right(extend),
        Action::MoveUp(extend) => app.buffers[app.active].ed.move_up(extend),
        Action::MoveDown(extend) => app.buffers[app.active].ed.move_down(extend),
        Action::MoveHome(extend) => app.buffers[app.active].ed.move_home(extend),
        Action::MoveEnd(extend) => app.buffers[app.active].ed.move_end(extend),
        Action::PageUp(extend) => app.buffers[app.active].ed.move_page_up(page_size, extend),
        Action::PageDown(extend) => app.buffers[app.active].ed.move_page_down(page_size, extend),
        Action::InsertNewline => app.buffers[app.active].ed.insert_newline(),
        Action::Backspace => app.buffers[app.active].ed.backspace(),
        Action::DeleteForward => app.buffers[app.active].ed.delete_forward(),
        // Con una selección de varias líneas, Tab indenta el bloque en vez de
        // reemplazarlo por un tabulador — que es lo que haría cualquier otra
        // inserción, y nunca es lo que se quiso.
        Action::InsertTab => {
            let ed = &mut app.buffers[app.active].ed;
            if ed.selection_spans_lines() {
                ed.indent_lines();
            } else {
                ed.insert_char('\t');
            }
        }
        Action::Unindent => app.buffers[app.active].ed.unindent_lines(),
        Action::SelectWord => {
            app.buffers[app.active].ed.status = if app.buffers[app.active].ed.select_word() {
                "Palabra seleccionada".to_string()
            } else {
                "No hay más palabras en esta línea".to_string()
            };
        }
        Action::SelectLine => {
            app.buffers[app.active].ed.select_line(app.last_was_select_line);
            app.buffers[app.active].ed.status = "Línea seleccionada".to_string();
        }
        Action::ExpandSelection => normal_expand_selection(app),
        Action::NormalDelete => normal_delete(app),
        Action::NormalChange => normal_change(app),
        Action::NormalYank => normal_yank(app),
        Action::NormalPaste => normal_paste(app),
        Action::InsertAt(at) => enter_insert(app, at),
        Action::OpenBelow => normal_open_below(app),
        Action::OpenAbove => normal_open_above(app),
        Action::EscapeKey => handle_escape(app),
        Action::OpenFilePrompt => {
            app.buffers[app.active].ed.mode = Mode::Prompt {
                kind: PromptKind::OpenFile,
                buffer: String::new(),
                label: "Abrir archivo: ".to_string(),
            };
        }
        Action::NextBuffer => switch_buffer(app, 1),
        Action::PrevBuffer => switch_buffer(app, -1),
        Action::CloseBuffer => {
            let multi = app.buffers.len() > 1;
            try_quit(&mut app.buffers[app.active].ed, multi);
        }
        Action::ToggleWrap => {
            let ed = &mut app.buffers[app.active].ed;
            ed.wrap = !ed.wrap;
            ed.col_offset = 0;
            ed.status = if ed.wrap {
                "Ajuste de línea activado".to_string()
            } else {
                "Ajuste de línea desactivado".to_string()
            };
        }
    }
}

/// Va al buffer siguiente (`delta == 1`) o anterior (`delta == -1`), dando la
/// vuelta en cualquiera de las dos puntas.
fn switch_buffer(app: &mut App, delta: i32) {
    let len = app.buffers.len();
    if len <= 1 {
        app.buffers[app.active].ed.status = "Un solo buffer abierto".to_string();
        return;
    }
    let cur = app.active as i32;
    app.active = (cur + delta).rem_euclid(len as i32) as usize;
    let label = app.buffers[app.active].tab_label();
    let n = app.active + 1;
    app.buffers[app.active].ed.status = format!("Buffer {n}/{len}: {label}");
}

/// Si ya hay un servidor de lenguaje corriendo (arrancado con el primer
/// archivo que lo necesitó) y este buffer es del mismo lenguaje, lo suma como
/// un documento más de ESE MISMO proceso — no se lanza un servidor nuevo por
/// cada archivo. Si no hay ninguno corriendo o es de otro lenguaje, el
/// buffer se queda sin LSP (el resaltado de sintaxis no depende de esto y
/// sigue andando igual).
fn try_attach_lsp(app: &mut App, idx: usize) {
    let Some(lang) = app.buffers[idx].lang else { return };
    let lang_id = lang_id_str(&lang);
    if app.lsp.is_none() || app.lsp_lang != Some(lang_id) {
        return;
    }
    let Some(path) = app.buffers[idx].ed.filename.clone() else { return };
    let uri = lsp::file_uri(&path);
    let text = app.buffers[idx].ed.rope.to_string();
    let opened = app
        .lsp
        .as_mut()
        .is_some_and(|c| c.did_open(&uri, lang_id, &text).is_ok());
    if opened {
        app.buffers[idx].doc_uri = Some(uri);
        app.buffers[idx].lsp_lang_id = Some(lang_id);
        app.buffers[idx].ed.status = format!("{} · {lang_id} LSP conectado", app.buffers[idx].ed.status);
    }
}

/// Abre `path_str` en un buffer nuevo y lo hace el activo. No bloquea para
/// preguntar si instalar un servidor de lenguaje (eso solo pasa al arrancar,
/// con el primer archivo) — si ya hay uno corriendo para este lenguaje se
/// suma a él; si no, el buffer arranca sin LSP.
fn open_file_into_new_buffer(app: &mut App, path_str: String) {
    let trimmed = path_str.trim();
    if trimmed.is_empty() {
        app.buffers[app.active].ed.status = "Cancelado".to_string();
        return;
    }
    match Buffer::open(Some(PathBuf::from(trimmed))) {
        Ok(mut new_buf) => {
            new_buf.ed.tab_width = app.theme.tab_width;
            if let Some(l) = &new_buf.lang {
                new_buf.ed.status = format!("{} — resaltado: {}", new_buf.ed.status, l.label());
            }
            app.buffers.push(new_buf);
            app.active = app.buffers.len() - 1;
            try_attach_lsp(app, app.active);
            let count = app.buffers.len();
            app.buffers[app.active].ed.status =
                format!("{} — {count} buffer(s) abierto(s)", app.buffers[app.active].ed.status);
        }
        Err(e) => app.buffers[app.active].ed.status = format!("No se pudo abrir \"{trimmed}\": {e}"),
    }
}

/// `Esc` es sensible al contexto: dentro de INSERT vuelve a NORMAL; si hay
/// cursores extra los descarta (en cualquier capa); si no hay nada de eso y
/// se está en NORMAL, limpia la selección de la primaria.
fn handle_escape(app: &mut App) {
    if app.buffers[app.active].ed.layer == Layer::ModalInsert {
        app.buffers[app.active].ed.layer = Layer::ModalNormal;
        app.buffers[app.active].ed.status = "NORMAL".to_string();
        return;
    }
    let had_secondary = !app.buffers[app.active].ed.secondary.is_empty();
    app.buffers[app.active].ed.secondary.clear();
    if app.buffers[app.active].ed.layer == Layer::ModalNormal {
        app.buffers[app.active].ed.selection_anchor = None;
    }
    app.buffers[app.active].ed.status = if had_secondary {
        "Cursores extra descartados".to_string()
    } else {
        "Selección limpiada".to_string()
    };
}

/// Puntaje de coincidencia difusa: `query` tiene que aparecer como
/// subsecuencia de `candidate` (sin importar mayúsculas), con puntos extra
/// por coincidencias seguidas o que empiezan justo al principio. `None` si
/// no hay coincidencia en absoluto.
fn fuzzy_score(query: &str, candidate: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let c: Vec<char> = candidate.to_lowercase().chars().collect();
    let mut qi = 0;
    let mut score = 0i32;
    let mut last_match: Option<usize> = None;
    for (ci, &ch) in c.iter().enumerate() {
        if qi < q.len() && ch == q[qi] {
            score += 10;
            match last_match {
                Some(prev) if ci == prev + 1 => score += 15,
                None if ci == 0 => score += 5,
                _ => {}
            }
            last_match = Some(ci);
            qi += 1;
        }
    }
    if qi == q.len() { Some(score) } else { None }
}

fn filtered_palette_indices(app: &App, query: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = app
        .palette_entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| fuzzy_score(query, &e.label).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

fn open_command_palette(app: &mut App) {
    app.buffers[app.active].ed.mode = Mode::Palette {
        query: String::new(),
        selected: 0,
    };
}

fn handle_palette_key(app: &mut App, key: KeyEvent) {
    let (mut query, mut selected) = match std::mem::replace(&mut app.buffers[app.active].ed.mode, Mode::Editing) {
        Mode::Palette { query, selected } => (query, selected),
        other => {
            app.buffers[app.active].ed.mode = other;
            return;
        }
    };

    match key.code {
        KeyCode::Esc => {
            app.buffers[app.active].ed.status = "Cancelado".to_string();
            return;
        }
        KeyCode::Backspace => {
            query.pop();
            selected = 0;
        }
        KeyCode::Up => {
            let filtered = filtered_palette_indices(app, &query);
            selected = selected
                .checked_sub(1)
                .unwrap_or(filtered.len().saturating_sub(1));
        }
        KeyCode::Down => {
            let filtered = filtered_palette_indices(app, &query);
            if !filtered.is_empty() {
                selected = (selected + 1) % filtered.len();
            }
        }
        KeyCode::Enter => {
            let filtered = filtered_palette_indices(app, &query);
            match filtered.get(selected) {
                Some(&idx) => {
                    run_palette_command(app, idx);
                    return;
                }
                None => {
                    // Sin coincidencias: la paleta se queda abierta para
                    // seguir corrigiendo la búsqueda, no se cierra sola.
                    app.buffers[app.active].ed.mode = Mode::Palette { query, selected };
                    return;
                }
            }
        }
        KeyCode::Char(c) => {
            query.push(c);
            selected = 0;
        }
        _ => {}
    }
    app.buffers[app.active].ed.mode = Mode::Palette { query, selected };
}

fn run_palette_command(app: &mut App, idx: usize) {
    let Some(kind) = app.palette_entries.get(idx).map(|e| e.kind) else {
        return;
    };
    match kind {
        CommandKind::Builtin(action) => {
            let page_size = app.text_area.height.max(1) as usize;
            dispatch_action(app, action, page_size);
        }
        CommandKind::SwitchProfile(name) => {
            if let Some(km) = keymap::profile_by_name(name) {
                app.buffers[app.active].ed.status = format!("Perfil de teclado: {}", km.label);
                app.keymap = km;
            }
        }
        CommandKind::Plugin(i) => run_plugin_command(app, i),
    }
}

fn run_plugin_command(app: &mut App, i: usize) {
    let Some(bridge) = app.plugins.as_ref() else {
        return;
    };
    let Some(cmd) = app.plugin_commands.get(i) else {
        return;
    };
    let filename = app.buffers[app.active]
        .ed
        .filename
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let line_count = app.buffers[app.active].ed.line_count();
    match bridge.invoke(cmd, &filename, line_count) {
        Ok((status, inserts)) => {
            for text in inserts {
                for c in text.chars() {
                    app.buffers[app.active].ed.insert_char(c);
                }
            }
            app.buffers[app.active].ed.status = status.unwrap_or_else(|| "Comando de plugin ejecutado".to_string());
        }
        Err(e) => app.buffers[app.active].ed.status = format!("Error en el plugin \"{}\": {e}", cmd.id),
    }
}

fn normal_delete(app: &mut App) {
    // "Borrar" corta: lo que se va a borrar (el rango si hay selección, si no
    // el carácter siguiente en cada cursor) queda guardado en el registro y
    // en el portapapeles del sistema antes de borrarlo — igual que la `d` de
    // Vim/Kakoune, recuperable con `p` después. Actúa sobre todos los
    // cursores a la vez, nunca es un no-op.
    if let Some(text) = app.buffers[app.active].ed.text_at_forward_delete_points() {
        set_clipboard(app, text);
    }
    app.buffers[app.active].ed.delete_at_each_selection();
    app.buffers[app.active].ed.status = "Cortado".to_string();
}

fn normal_change(app: &mut App) {
    // Igual que `d`: lo que había en la selección queda guardado antes de
    // borrarlo (si no había selección, no hay nada que guardar).
    if let Some(text) = app.buffers[app.active].ed.selected_text() {
        set_clipboard(app, text);
    }
    app.buffers[app.active].ed.delete_selection_action();
    app.buffers[app.active].ed.layer = Layer::ModalInsert;
    app.buffers[app.active].ed.status = "INSERT".to_string();
}

fn normal_yank(app: &mut App) {
    match app.buffers[app.active].ed.selected_text() {
        Some(text) => {
            set_clipboard(app, text);
            app.buffers[app.active].ed.status = "Copiado".to_string();
        }
        None => app.buffers[app.active].ed.status = "Nada seleccionado".to_string(),
    }
}

fn normal_paste(app: &mut App) {
    paste_from_clipboard(app);
}

/// Guarda `text` en el registro interno y, si hay portapapeles del sistema
/// disponible, ahí también — así lo que se copia/corta en Flint se puede
/// pegar en cualquier otra aplicación.
fn set_clipboard(app: &mut App, text: String) {
    if let Some(cb) = app.clipboard.as_mut()
        && let Err(e) = cb.set_text(text.clone())
    {
        app.buffers[app.active].ed.status = format!("Copiado (solo interno — el portapapeles del sistema falló: {e})");
    }
    app.buffers[app.active].ed.register = Some(text);
}

/// El texto a pegar: el portapapeles del sistema si está disponible y tiene
/// algo (así se puede pegar lo copiado en cualquier otra app), si no el
/// registro interno de Flint.
fn get_clipboard_text(app: &mut App) -> Option<String> {
    if let Some(cb) = app.clipboard.as_mut()
        && let Ok(text) = cb.get_text()
        && !text.is_empty()
    {
        return Some(text);
    }
    app.buffers[app.active].ed.register.clone()
}

fn paste_from_clipboard(app: &mut App) {
    let Some(text) = get_clipboard_text(app) else {
        app.buffers[app.active].ed.status = "Nada que pegar (portapapeles vacío)".to_string();
        return;
    };
    // Sin condición: delete_selection_action ya revisa todas las selecciones
    // (no solo la primaria) y no hace nada si ninguna tiene rango.
    app.buffers[app.active].ed.delete_selection_action();
    for c in text.chars() {
        app.buffers[app.active].ed.insert_char(c);
    }
    app.buffers[app.active].ed.status = "Pegado".to_string();
}

/// Colapsa TODAS las selecciones (no solo la primaria) al punto donde debe
/// empezar a escribirse, y entra a INSERT. Si solo tocara la primaria, un
/// multi-cursor armado con `w`/`^D` perdería sus cursores extra al escribir.
fn enter_insert(app: &mut App, at: keymap::InsertAt) {
    let mut sels = app.buffers[app.active].ed.selections_snapshot();
    for sel in sels.iter_mut() {
        let (a, c) = (sel.anchor, sel.cursor);
        let (range_start, range_end) = if (a.line, a.col) <= (c.line, c.col) {
            (a, c)
        } else {
            (c, a)
        };
        let new_pos = match at {
            keymap::InsertAt::SelectionStart => range_start,
            keymap::InsertAt::SelectionEnd => range_end,
            keymap::InsertAt::LineStart => editor::Position {
                line: sel.cursor.line,
                col: 0,
            },
            keymap::InsertAt::LineEnd => editor::Position {
                line: sel.cursor.line,
                col: app.buffers[app.active].ed.line_char_len(sel.cursor.line),
            },
        };
        *sel = editor::Selection {
            anchor: new_pos,
            cursor: new_pos,
        };
    }
    app.buffers[app.active].ed.apply_selections(sels);
    app.buffers[app.active].ed.layer = Layer::ModalInsert;
    app.buffers[app.active].ed.status = "INSERT".to_string();
}

fn normal_open_below(app: &mut App) {
    app.buffers[app.active].ed.move_end(false);
    app.buffers[app.active].ed.selection_anchor = None;
    app.buffers[app.active].ed.insert_newline();
    app.buffers[app.active].ed.layer = Layer::ModalInsert;
    app.buffers[app.active].ed.status = "INSERT".to_string();
}

fn normal_open_above(app: &mut App) {
    app.buffers[app.active].ed.move_home(false);
    app.buffers[app.active].ed.selection_anchor = None;
    app.buffers[app.active].ed.insert_newline();
    app.buffers[app.active].ed.move_up(false);
    app.buffers[app.active].ed.layer = Layer::ModalInsert;
    app.buffers[app.active].ed.status = "INSERT".to_string();
}

/// Selección estructural: expande cada selección activa (primaria y
/// secundarias) al nodo del árbol de tree-sitter que la contiene — o a su
/// nodo padre si ya coincide con uno exacto. Cada cursor sube por su propia
/// rama del árbol de forma independiente.
fn normal_expand_selection(app: &mut App) {
    let Some(highlighter) = app.buffers[app.active].highlighter.as_ref() else {
        app.buffers[app.active].ed.status = "Sin árbol de sintaxis para este archivo".to_string();
        return;
    };
    let text = app.buffers[app.active].ed.rope.to_string();
    let Some(tree) = highlighter.parse(&text) else {
        app.buffers[app.active].ed.status = "No se pudo analizar el archivo".to_string();
        return;
    };

    let mut sels = app.buffers[app.active].ed.selections_snapshot();
    let len_chars = app.buffers[app.active].ed.rope.len_chars();
    let mut changed = 0;
    for sel in sels.iter_mut() {
        let (start_char, end_char) = app.buffers[app.active].ed.selection_char_range(*sel);
        let end_char = end_char.max(start_char + 1).min(len_chars);
        let start_byte = app.buffers[app.active].ed.rope.char_to_byte(start_char);
        let end_byte = app.buffers[app.active].ed.rope.char_to_byte(end_char);
        if let Some((nb_start, nb_end)) = highlight::expand_selection(&tree, start_byte, end_byte) {
            let new_start = app.buffers[app.active].ed.position_from_char_idx(app.buffers[app.active].ed.rope.byte_to_char(nb_start));
            let new_end = app.buffers[app.active].ed.position_from_char_idx(app.buffers[app.active].ed.rope.byte_to_char(nb_end));
            *sel = editor::Selection {
                anchor: new_start,
                cursor: new_end,
            };
            changed += 1;
        }
    }
    app.buffers[app.active].ed.apply_selections(sels);
    app.buffers[app.active].ed.status = if changed > 0 {
        "Selección expandida al nodo padre".to_string()
    } else {
        "No se pudo expandir la selección".to_string()
    };
}

fn trigger_completion(app: &mut App) {
    if app.buffers[app.active].lsp_lang_id.is_none() {
        app.buffers[app.active].ed.status = "Sin servidor de lenguaje para autocompletar".to_string();
        return;
    }
    let line = app.buffers[app.active].ed.cursor.line;
    // LSP mide columnas en unidades UTF-16, no en caracteres — no son lo
    // mismo con texto no-ASCII antes del cursor en la línea.
    let character = app.buffers[app.active]
        .ed
        .char_col_to_utf16(line, app.buffers[app.active].ed.cursor.col);
    let (trigger_pos, prefix) = app.buffers[app.active].ed.identifier_prefix_before_cursor();
    let trigger = (trigger_pos.line, trigger_pos.col);
    let buffer = app.active;
    let (Some(client), Some(uri)) = (app.lsp.as_mut(), app.buffers[app.active].doc_uri.as_ref()) else {
        app.buffers[app.active].ed.status = "Sin servidor de lenguaje para autocompletar".to_string();
        return;
    };
    match client.request_completion(uri, line, character, buffer, trigger, prefix) {
        Ok(_) => app.buffers[app.active].ed.status = "Buscando sugerencias…".to_string(),
        Err(e) => app.buffers[app.active].ed.status = format!("Error al pedir autocompletado: {e}"),
    }
}

/// Índices de `items` que coinciden con `prefix` (lo tipeado desde que se
/// abrió el popup), en el mismo orden de puntaje difuso que usa la paleta de
/// comandos — reutiliza `fuzzy_score` en vez de un mecanismo aparte.
fn filtered_completion_indices(items: &[editor::CompletionEntry], prefix: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy_score(prefix, &it.label).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// El popup de autocompletado sigue filtrando mientras se escribe (como la
/// paleta de comandos): las letras que se tipean se insertan en el buffer *y*
/// se usan para filtrar la lista. Al aceptar, se borra todo lo tipeado desde
/// que se abrió el popup (`trigger` → cursor actual) y se inserta el texto
/// definitivo de la entrada elegida — así da igual qué tan aproximado fue el
/// filtro tipeado, el resultado final es siempre exactamente lo que ofreció
/// el servidor, sin texto duplicado ni sobrante.
fn handle_completion_key(app: &mut App, key: KeyEvent) {
    let (items, mut selected, trigger, mut prefix) =
        match std::mem::replace(&mut app.buffers[app.active].ed.mode, Mode::Editing) {
            Mode::Completion { items, selected, trigger, prefix } => (items, selected, trigger, prefix),
            other => {
                app.buffers[app.active].ed.mode = other;
                return;
            }
        };

    match key.code {
        KeyCode::Esc => {
            app.buffers[app.active].ed.status = "Cancelado".to_string();
            return;
        }
        KeyCode::Up => {
            let filtered = filtered_completion_indices(&items, &prefix);
            selected = selected.checked_sub(1).unwrap_or(filtered.len().saturating_sub(1));
        }
        KeyCode::Down => {
            let filtered = filtered_completion_indices(&items, &prefix);
            if !filtered.is_empty() {
                selected = (selected + 1) % filtered.len();
            }
        }
        KeyCode::Enter | KeyCode::Tab => {
            let filtered = filtered_completion_indices(&items, &prefix);
            let Some(&idx) = filtered.get(selected) else {
                app.buffers[app.active].ed.status = "Sin coincidencias".to_string();
                app.buffers[app.active].ed.mode = Mode::Completion { items, selected, trigger, prefix };
                return;
            };
            let entry = &items[idx];
            let text = entry.insert_text.clone().unwrap_or_else(|| entry.label.clone());
            let label = entry.label.clone();
            let ed = &mut app.buffers[app.active].ed;
            ed.selection_anchor = Some(trigger);
            ed.secondary.clear();
            ed.delete_selection_action();
            for c in text.chars() {
                ed.insert_char(c);
            }
            ed.status = format!("Insertado: {label}");
            return;
        }
        KeyCode::Backspace => {
            if prefix.pop().is_none() {
                app.buffers[app.active].ed.status = "Cancelado".to_string();
                return;
            }
            app.buffers[app.active].ed.backspace();
            selected = 0;
        }
        KeyCode::Char(c) => {
            app.buffers[app.active].ed.insert_char(c);
            prefix.push(c);
            selected = 0;
        }
        _ => {}
    }
    app.buffers[app.active].ed.mode = Mode::Completion { items, selected, trigger, prefix };
}

fn handle_prompt_key(app: &mut App, key: KeyEvent) {
    let ed = &mut app.buffers[app.active].ed;
    let (kind, mut buffer, label) = match std::mem::replace(&mut ed.mode, Mode::Editing) {
        Mode::Prompt { kind, buffer, label } => (kind, buffer, label),
        other => {
            ed.mode = other;
            return;
        }
    };

    match key.code {
        KeyCode::Esc => {
            app.buffers[app.active].ed.status = "Cancelado".to_string();
        }
        KeyCode::Backspace => {
            buffer.pop();
            app.buffers[app.active].ed.mode = Mode::Prompt { kind, buffer, label };
        }
        KeyCode::Enter => {
            submit_prompt(app, kind, buffer);
        }
        KeyCode::Char(c) => {
            if let PromptKind::QuitConfirm = &kind {
                match c {
                    's' | 'S' => try_save(&mut app.buffers[app.active].ed, true),
                    'n' | 'N' => app.buffers[app.active].ed.should_quit = true,
                    _ => app.buffers[app.active].ed.mode = Mode::Prompt { kind, buffer, label },
                }
            } else {
                buffer.push(c);
                app.buffers[app.active].ed.mode = Mode::Prompt { kind, buffer, label };
            }
        }
        _ => {
            app.buffers[app.active].ed.mode = Mode::Prompt { kind, buffer, label };
        }
    }
}

/// La mayoría de los `PromptKind` solo tocan el `Editor` activo; `OpenFile` y
/// `SaveAs` son la excepción — abrir un archivo suma un `Buffer` entero a
/// `App`, y "Guardar como" puede cambiar la extensión (y con ella el
/// lenguaje detectado), así que ambos se resuelven acá, con acceso completo
/// a `App`, antes de delegar el resto sin cambios.
fn submit_prompt(app: &mut App, kind: PromptKind, buffer: String) {
    match kind {
        PromptKind::OpenFile => open_file_into_new_buffer(app, buffer),
        PromptKind::SaveAs { then_quit } => handle_save_as(app, then_quit, buffer),
        other => submit_prompt_editor(&mut app.buffers[app.active].ed, other, buffer),
    }
}

/// Guarda con el nombre nuevo y, si la extensión cambió (o el buffer nunca
/// tuvo nombre, así que nunca tuvo lenguaje detectado), recalcula resaltado
/// y conexión LSP sin necesitar reabrir Flint.
fn handle_save_as(app: &mut App, then_quit: bool, path_str: String) {
    let trimmed = path_str.trim();
    if trimmed.is_empty() {
        app.buffers[app.active].ed.status = "Nombre vacío, cancelado".to_string();
        return;
    }
    let new_path = PathBuf::from(trimmed);
    app.buffers[app.active].ed.filename = Some(new_path.clone());
    match app.buffers[app.active].ed.save() {
        Ok(()) => {
            app.buffers[app.active].ed.status = "Guardado".to_string();
            redetect_language(app, app.active, &new_path);
            if then_quit {
                app.buffers[app.active].ed.should_quit = true;
            }
        }
        Err(e) => app.buffers[app.active].ed.status = format!("Error al guardar: {e}"),
    }
}

/// Si el lenguaje que le corresponde a `path` cambió respecto al que tenía
/// este buffer (extensión distinta en un "Guardar como", o un buffer que
/// nunca tuvo nombre y por lo tanto nunca tuvo lenguaje), recalcula el
/// resaltado de sintaxis y — si corresponde — la conexión LSP: cierra el
/// documento viejo (si había uno abierto contra un servidor) y trata de
/// sumarse a un servidor ya corriendo para el lenguaje nuevo, igual que al
/// abrir un buffer con `Ctrl+O`.
fn redetect_language(app: &mut App, idx: usize, path: &Path) {
    let new_lang = highlight::lang_for_path(path);
    let same = match (app.buffers[idx].lang, new_lang) {
        (Some(a), Some(b)) => lang_id_str(&a) == lang_id_str(&b),
        (None, None) => true,
        _ => false,
    };
    if same {
        return;
    }
    if let (Some(uri), Some(client)) = (app.buffers[idx].doc_uri.take(), app.lsp.as_mut()) {
        let _ = client.did_close(&uri);
    }
    app.buffers[idx].lsp_lang_id = None;
    app.buffers[idx].lsp_synced_version = 0;
    app.buffers[idx].highlighter = new_lang.as_ref().and_then(highlight::LanguageHighlighter::new);
    app.buffers[idx].lang = new_lang;
    app.buffers[idx].ed.highlights_dirty = true;
    if let Some(l) = &app.buffers[idx].lang {
        app.buffers[idx].ed.status = format!("{} — resaltado: {}", app.buffers[idx].ed.status, l.label());
    }
    try_attach_lsp(app, idx);
}

fn submit_prompt_editor(ed: &mut Editor, kind: PromptKind, buffer: String) {
    match kind {
        PromptKind::Search => {
            let query = if buffer.trim().is_empty() {
                ed.last_search.clone().unwrap_or_default()
            } else {
                buffer
            };
            if query.is_empty() {
                ed.status = "Nada que buscar".to_string();
            } else {
                ed.last_search = Some(query.clone());
                if ed.find_next(&query) {
                    ed.status = format!("Encontrado: \"{query}\"");
                } else {
                    ed.status = format!("No se encontró: \"{query}\"");
                }
            }
        }
        PromptKind::ReplaceSearch => {
            if buffer.trim().is_empty() {
                ed.status = "Cancelado".to_string();
            } else {
                let label = format!("Reemplazar \"{buffer}\" con: ");
                ed.mode = Mode::Prompt {
                    kind: PromptKind::ReplaceWith { search: buffer },
                    buffer: String::new(),
                    label,
                };
            }
        }
        PromptKind::ReplaceWith { search } => {
            let count = ed.replace_all(&search, &buffer);
            ed.status = format!("{count} reemplazo(s) hecho(s)");
        }
        PromptKind::SearchRegex => {
            let pattern = if buffer.trim().is_empty() {
                ed.last_search.clone().unwrap_or_default()
            } else {
                buffer
            };
            if pattern.is_empty() {
                ed.status = "Nada que buscar".to_string();
            } else {
                ed.last_search = Some(pattern.clone());
                match ed.find_next_regex(&pattern) {
                    Ok(true) => ed.status = format!("Encontrado: /{pattern}/"),
                    Ok(false) => ed.status = format!("No se encontró: /{pattern}/"),
                    Err(e) => ed.status = format!("Regex inválida: {e}"),
                }
            }
        }
        PromptKind::ReplaceRegexSearch => {
            if buffer.trim().is_empty() {
                ed.status = "Cancelado".to_string();
            } else if let Err(e) = Regex::new(&buffer) {
                ed.status = format!("Regex inválida: {e}");
            } else {
                let label = format!("Reemplazar /{buffer}/ con: ");
                ed.mode = Mode::Prompt {
                    kind: PromptKind::ReplaceRegexWith { pattern: buffer },
                    buffer: String::new(),
                    label,
                };
            }
        }
        PromptKind::ReplaceRegexWith { pattern } => match ed.replace_all_regex(&pattern, &buffer) {
            Ok(count) => ed.status = format!("{count} reemplazo(s) hecho(s)"),
            Err(e) => ed.status = format!("Regex inválida: {e}"),
        },
        PromptKind::QuitConfirm => {
            ed.status = "Cancelado".to_string();
        }
        PromptKind::OpenFile | PromptKind::SaveAs { .. } => {
            unreachable!("interceptados antes, en submit_prompt")
        }
    }
}

fn try_save(ed: &mut Editor, then_quit: bool) {
    if ed.filename.is_some() {
        match ed.save() {
            Ok(()) => {
                ed.status = "Guardado".to_string();
                if then_quit {
                    ed.should_quit = true;
                }
            }
            Err(e) => ed.status = format!("Error al guardar: {e}"),
        }
    } else {
        ed.mode = Mode::Prompt {
            kind: PromptKind::SaveAs { then_quit },
            buffer: String::new(),
            label: "Guardar como: ".to_string(),
        };
    }
}

/// Con un solo buffer abierto esto sale del programa; con varios, cierra
/// nomás el activo (el proceso solo termina cuando se cierra el último) — el
/// texto del prompt lo aclara para no confundir "cerrar pestaña" con "salir".
fn try_quit(ed: &mut Editor, multi_buffer: bool) {
    if ed.dirty {
        let action = if multi_buffer { "cerrar este buffer" } else { "salir" };
        ed.mode = Mode::Prompt {
            kind: PromptKind::QuitConfirm,
            buffer: String::new(),
            label: format!("¿Guardar cambios antes de {action}? (s = sí, n = no, Esc = cancelar): "),
        };
    } else {
        ed.should_quit = true;
    }
}

fn handle_mouse(app: &mut App, m: MouseEvent) {
    if !matches!(app.buffers[app.active].ed.mode, Mode::Editing) {
        return;
    }
    if let (MouseEventKind::Down(MouseButton::Left), Some(tabs_area)) = (m.kind, app.tabs_area)
        && m.row == tabs_area.y
    {
        let tab_labels: Vec<String> = app.buffers.iter().map(Buffer::tab_label).collect();
        if let Some(idx) = ui::tab_at_column(&tab_labels, tabs_area, m.column) {
            app.active = idx;
            return;
        }
    }
    let text_area = app.text_area;
    let ed = &mut app.buffers[app.active].ed;
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(pos) = ui::screen_to_pos(ed, text_area, m.column, m.row) {
                if m.modifiers.contains(KeyModifiers::ALT) {
                    // Alt+clic agrega un cursor nuevo, sin tocar los que ya había.
                    ed.secondary.push(editor::Selection {
                        anchor: pos,
                        cursor: pos,
                    });
                } else {
                    // Un clic normal vuelve a un solo cursor, donde se hizo clic.
                    ed.cursor = pos;
                    ed.selection_anchor = None;
                    ed.secondary.clear();
                }
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some(pos) = ui::screen_to_pos(ed, text_area, m.column, m.row) {
                if ed.selection_anchor.is_none() {
                    ed.selection_anchor = Some(ed.cursor);
                }
                ed.cursor = pos;
            }
        }
        MouseEventKind::ScrollDown => {
            let max_line = ed.line_count().saturating_sub(1);
            ed.row_offset = (ed.row_offset + 3).min(max_line);
        }
        MouseEventKind::ScrollUp => {
            ed.row_offset = ed.row_offset.saturating_sub(3);
        }
        _ => {}
    }
}
