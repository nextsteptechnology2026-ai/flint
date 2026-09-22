mod busqueda;
mod clipboard;
mod config;
mod editor;
mod highlight;
mod markdown;
mod keymap;
mod lsp;
mod plugins;
mod saltos;
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
/// Cada cuánto se revisa si `theme.toml` cambió en disco, para recargarlo en
/// caliente — un segundo es seguido para sentirse "en vivo" sin convertir
/// cada vuelta del loop principal en un `stat()`.
const THEME_RELOAD_INTERVAL: Duration = Duration::from_secs(1);
/// Cada cuánto se escribe un respaldo de lo que está sin guardar. Es el peor
/// caso de lo que se puede perder en una caída; más seguido que esto sería
/// escribir en disco durante el tipeo, que se nota.
const BACKUP_INTERVAL: Duration = Duration::from_secs(8);

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
        let mut buffer = Buffer {
            ed: Editor::open(path)?,
            lang: None,
            highlighter: None,
            doc_uri: None,
            lsp_lang_id: None,
            lsp_synced_version: 0,
        };
        buffer.detectar_lenguaje();
        Ok(buffer)
    }

    /// Primero la extensión, que es barata y no se equivoca; si el archivo
    /// no tiene (un script llamado `deploy` a secas), lo dice su shebang. Se
    /// vuelve a llamar después de cargar los plugins, que pueden traer
    /// lenguajes nuevos: el primer archivo se abre antes que ellos.
    fn detectar_lenguaje(&mut self) {
        let ed = &mut self.ed;
        self.lang = ed
            .filename
            .as_ref()
            .and_then(|p| highlight::lang_for_path(p))
            .or_else(|| {
                let primera: String = ed.rope.line(0).chars().take(200).collect();
                highlight::lang_for_first_line(&primera)
            });
        self.highlighter = self.lang.as_ref().and_then(highlight::LanguageHighlighter::new);
        // Lo que depende del lenguaje y no de la configuración del usuario
        // se fija acá, donde el lenguaje recién se conoce.
        ed.indent_after_colon = self.lang.is_some_and(|l| l.indents_after_colon());
        ed.highlights_dirty = true;
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
    /// El servidor que se lanzó y todavía no contestó el `initialize`.
    lsp_arrancando: Option<ArranqueLsp>,
    /// El lenguaje que sirve `lsp`, si hay uno corriendo — para saber si un
    /// buffer nuevo del mismo lenguaje puede sumarse al mismo servidor en vez
    /// de necesitar uno propio.
    lsp_lang: Option<&'static str>,
    /// Si `lsp` anunció soporte de sincronización incremental
    /// (`textDocumentSync.change == 2`) al iniciarse — si no, siempre se
    /// manda el documento completo, que es lo único universalmente soportado.
    lsp_incremental: bool,
    /// Si el servidor anunció que sabe ir a la definición y renombrar. Se
    /// pregunta antes de pedirlo: mandar una petición que el servidor no
    /// atiende deja al usuario esperando una respuesta que no va a llegar.
    lsp_definition: bool,
    lsp_rename: bool,
    lsp_hover: bool,
    lsp_format: bool,
    lsp_firma: bool,
    /// La ayuda de firmas abierta, si hay una. No es un modo: se sigue
    /// escribiendo con ella a la vista.
    firma: Option<FirmaAbierta>,
    /// Salta el margen de espera de la sincronización. Se prende solo para
    /// las peticiones que dependen de que el servidor tenga el texto de
    /// ahora mismo (ir a la definición, renombrar).
    lsp_forzar_sync: bool,
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
    /// Teclas que los plugins ataron a un comando propio. Van aparte del
    /// `Keymap`, que solo sabe de `Action`: acá el valor es el índice del
    /// comando en `plugin_commands`. Se consultan antes que el perfil, así
    /// que un plugin puede pisar una tecla de Flint a propósito.
    plugin_keys: HashMap<(keymap::KeyChord, bool), usize>,
    theme: theme::Theme,
    /// Ruta del `theme.toml` en uso, si vino de un archivo real (no la
    /// paleta de fábrica) — para poder recargarlo en caliente.
    theme_path: Option<PathBuf>,
    theme_mtime: Option<std::time::SystemTime>,
    /// Cuándo toca volver a mirar la fecha de modificación del archivo de
    /// tema — revisarlo en cada vuelta del loop sería un `stat()` de más
    /// por cada tecla; una vez por segundo alcanza para sentirse "en vivo".
    theme_next_check: Instant,
    /// La configuración leída al arrancar: hace falta después del arranque
    /// para resolver las opciones de cada archivo que se abra, y para volver
    /// a resolverlas cuando el tema se recarga en caliente.
    config: config::Config,
    /// `None` si no se pudo abrir el portapapeles del sistema al arrancar
    /// (sin servidor gráfico, por ejemplo) — ahí copiar sigue saliendo por
    /// OSC 52 hacia la terminal, y pegar cae al registro interno.
    clipboard: Option<arboard::Clipboard>,
    /// Qué caminos tiene permitido usar `set_clipboard`, según `[options]`.
    clipboard_mode: clipboard::Mode,
    /// Las acciones que se están grabando, si hay una macro en curso.
    /// Grabar es acumular lo que pasa por el despachador, así que una macro
    /// repite exactamente lo mismo que hizo el teclado, tipeo incluido.
    macro_recording: Option<Vec<Action>>,
    /// La última macro terminada, lista para repetirse.
    macro_last: Vec<Action>,
    /// Las rutas que ofrece el buscador de archivos. Se arma al abrirlo y no
    /// antes: recorrer el proyecto al arrancar retrasaría la primera pantalla
    /// por algo que quizás no se usa, y armarlo cada vez mantiene la lista al
    /// día sin tener que vigilar el disco.
    file_index: Vec<String>,
    /// El proyecto leído a memoria mientras está abierta la búsqueda de texto
    /// (Ctrl+N), la raíz de la que salen sus rutas, y lo que encontró la
    /// última consulta. Se cargan al abrirla y se vacían al cerrarla.
    search_files: Vec<busqueda::Archivo>,
    search_root: PathBuf,
    /// Si el proyecto no entró entero en el tope de lectura.
    search_partial: bool,
    search_hits: Vec<busqueda::Coincidencia>,
    /// A dónde vuelve Alt+← y a dónde avanza Alt+→.
    saltos: saltos::ListaSaltos,
    /// El hover abierto, ya dibujado: Markdown con el código resaltado. Se
    /// arma una vez al llegar la respuesta y no en cada frame.
    hover_lines: Vec<ratatui::text::Line<'static>>,
    /// La vista previa ya renderizada y de qué estado salió
    /// (buffer, versión del contenido, ancho). Renderizar Markdown y
    /// resaltar sus cercos de código es caro comparado con dibujar, así que
    /// se rehace solo cuando cambió algo de lo que depende, no en cada frame.
    preview_lines: Vec<ratatui::text::Line<'static>>,
    preview_key: Option<(usize, u64, u16)>,
    /// Si se están corriendo los manejadores de un evento de plugin. Lo que
    /// pidan (un `flint.action("save")` en un manejador de guardado, por
    /// ejemplo) no vuelve a disparar eventos: sería un bucle.
    en_evento: bool,
}

/// El identificador que usa LSP para este lenguaje. Sale de la tabla de
/// `highlight`, que es donde vive todo lo que Flint sabe de un lenguaje.
fn lang_id_str(lang: &highlight::Lang) -> &'static str {
    lang.id()
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
        ("Autocompletar", Action::TriggerCompletion),
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
        ("Vista previa de Markdown (^E)", Action::TogglePreview),
        ("Indentar líneas (Tab)", Action::InsertTab),
        ("Des-indentar líneas (Shift+Tab)", Action::Unindent),
        ("Comentar/descomentar líneas (^K)", Action::ToggleComment),
        ("Saltar al paréntesis/llave que hace pareja", Action::JumpMatchingBracket),
        ("Abrir archivo del proyecto… (^T)", Action::FindFilePrompt),
        ("Buscar texto en el proyecto… (^N)", Action::SearchProjectPrompt),
        ("Ir a la línea…", Action::GotoLinePrompt),
        ("Ir a la definición (^] o F12)", Action::GotoDefinition),
        ("Renombrar el símbolo… (F6)", Action::RenamePrompt),
        ("Qué es esto: tipo y documentación (F1)", Action::Hover),
        ("Parámetros de la llamada (ayuda de firmas)", Action::SignatureHelp),
        ("Formatear el archivo (Alt+Shift+F)", Action::Format),
        ("Volver al lugar anterior (Alt+←)", Action::JumpBack),
        ("Avanzar al lugar siguiente (Alt+→)", Action::JumpForward),
        ("Grabar/terminar macro (^U)", Action::MacroRecord),
        ("Repetir la macro (^B)", Action::MacroPlay),
    ];
    // Cada renglón lleva además el nombre estable de su acción: es el mismo
    // que se escribe en `[keys]` en config.toml, así que la paleta sirve de
    // referencia para remapear sin salir del editor, y buscar "save"
    // encuentra "Guardar".
    let mut entries: Vec<PaletteEntry> = items
        .iter()
        .map(|&(label, action)| PaletteEntry {
            label: format!("{label} · {}", action.name()),
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
    "    Un solo archivo por línea de órdenes. Para abrir más, Ctrl+O o Ctrl+T ya estando adentro.\n",
    "\n",
    "OPCIONES:\n",
    "    --profile <flint|vim|emacs>   Perfil de atajos de teclado (por defecto: flint)\n",
    "    --theme <ruta>                 Archivo de tema .toml (por defecto: ~/.config/flint/theme.toml si existe)\n",
    "    --config <ruta>                Archivo de configuración .toml (por defecto: ~/.config/flint/config.toml si existe)\n",
    "    --actions                      Lista los nombres de acción para la sección [keys] y sale\n",
    "    -h, --help                     Muestra esta ayuda y sale\n",
    "    -V, --version                  Muestra la versión y sale\n",
    "\n",
    "ATAJOS PRINCIPALES (modo directo, perfil por defecto):\n",
    "    Ctrl+S  Guardar         Ctrl+Q  Salir            Ctrl+F  Buscar\n",
    "    Ctrl+R  Reemplazar      Ctrl+Z  Deshacer          Ctrl+Y  Rehacer\n",
    "    Ctrl+P  Paleta de comandos      Ctrl+Espacio  Autocompletar (LSP)\n",
    "    Ctrl+G  Siguiente diagnóstico   Ctrl+D  +cursor en la siguiente aparición\n",
    "    Ctrl+C/X/V  Copiar/Cortar/Pegar (portapapeles del sistema)\n",
    "    Ctrl+L  Alternar ajuste de línea       Ctrl+K  Comentar/descomentar\n",
    "    Ctrl+U  Grabar/terminar macro          Ctrl+B  Repetir la macro\n",
    "    Ctrl+E  Vista previa de Markdown (solo en .md/.markdown)\n",
    "    Ctrl+] o F12   Ir a la definición (LSP)      F6  Renombrar el símbolo (LSP)\n",
    "    F1      Tipo y documentación de lo que está bajo el cursor (LSP)\n",
    "    Alt+Shift+F   Formatear el archivo (LSP; format_on_save lo hace al guardar)\n",
    "    Alt+← / Alt+→   Volver al lugar anterior / avanzar (después de un salto)\n",
    "    Tab / Shift+Tab   Indentar / des-indentar el bloque seleccionado\n",
    "    Shift+flechas     Seleccionar (también Shift+Inicio/Fin/RePág/AvPág)\n",
    "    F2      Activar/desactivar la capa modal (NORMAL/INSERT)\n",
    "    Alt+clic  Agregar un cursor donde se hace clic\n",
    "\n",
    "BUFFERS (varios archivos a la vez):\n",
    "    Ctrl+O  Abrir archivo (en un buffer nuevo)     Ctrl+W  Cerrar buffer actual\n",
    "    Ctrl+T  Buscador difuso de archivos del proyecto (abre en un buffer nuevo)\n",
    "    Ctrl+N  Buscar texto en todos los archivos del proyecto\n",
    "    Ctrl+PageDown/PageUp  Siguiente/anterior buffer\n",
    "    La barra de pestañas responde al mouse: un clic cambia de buffer.\n",
    "\n",
    "CAPA MODAL — NORMAL (tras F2):\n",
    "    h/j/k/l   Mover el cursor (las flechas también andan)\n",
    "    w/x   Seleccionar palabra/línea   n     Expandir selección (sintaxis)\n",
    "    m     Saltar al delimitador que hace pareja\n",
    "    D     Ir a la definición          r     Renombrar el símbolo\n",
    "    K     Tipo y documentación (LSP)   =     Formatear el archivo (LSP)\n",
    "    d/c   Borrar/Cambiar              y/p   Copiar/Pegar\n",
    "    i/a/I/A   Insertar (selección/línea)     o/O   Abrir línea abajo/arriba\n",
    "    u/U   Deshacer/Rehacer            Esc   Deseleccionar / volver\n",
    "\n",
    "SOLO DESDE LA PALETA (Ctrl+P), sin tecla propia:\n",
    "    \"Ir a la línea…\", \"Buscar (regex)…\", \"Reemplazar (regex)…\" y cambiar\n",
    "    el perfil de teclado. Cada renglón muestra el nombre de su acción, que es\n",
    "    el que se escribe en [keys] en config.toml para reasignarle una tecla.\n",
    "\n",
    "Manual completo (temas, plugins, perfiles, todos los atajos): ver MANUAL.md\n",
    "en el repositorio del proyecto.\n",
);

/// Lo que se pidió desde la línea de comandos. Perfil y tema son `Option`
/// porque el archivo de configuración también los puede fijar: `None` acá
/// significa "no lo pidieron por bandera", no "usá el default" — la bandera
/// gana sobre la configuración, y la configuración sobre el default.
struct Args {
    path: Option<PathBuf>,
    profile: Option<String>,
    theme: Option<PathBuf>,
    config: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut out = Args {
        path: None,
        profile: None,
        theme: None,
        config: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--profile" {
            if let Some(v) = args.next() {
                out.profile = Some(v);
            }
        } else if arg == "--theme" {
            if let Some(v) = args.next() {
                out.theme = Some(PathBuf::from(v));
            }
        } else if arg == "--config" {
            if let Some(v) = args.next() {
                out.config = Some(PathBuf::from(v));
            }
        } else if arg == "--actions" {
            println!("Nombres de acción para [keys] y [modal_keys] en config.toml:");
            for name in keymap::all_action_names() {
                println!("    {name}");
            }
            println!("\n\"none\" como valor desata la tecla en vez de asignarle una acción.");
            std::process::exit(0);
        } else if arg == "-h" || arg == "--help" {
            print!("{HELP_TEXT}");
            std::process::exit(0);
        } else if arg == "-V" || arg == "--version" {
            println!("flint {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        } else {
            out.path = Some(PathBuf::from(arg));
        }
    }
    out
}

/// Las opciones que le tocan a un archivo: la configuración decide, y lo que
/// la configuración no menciona sale del tema (el ancho de tabulación) o de
/// los valores de fábrica.
fn resolve_options(
    cfg: &config::Config,
    theme: &theme::Theme,
    path: Option<&Path>,
) -> config::Options {
    let fallback = config::Options {
        tab_width: theme.tab_width,
        ..config::Options::default()
    };
    cfg.options_for(path, &fallback)
}

fn apply_options(ed: &mut Editor, o: &config::Options) {
    ed.tab_width = o.tab_width;
    ed.indent_with_spaces = o.indent_with_spaces;
    ed.wrap = o.wrap;
    ed.trim_on_save = o.trim_trailing_whitespace;
    ed.auto_close = o.auto_close_brackets;
    ed.format_on_save = o.format_on_save;
}

/// El comando del servidor de lenguaje: primero lo que diga `[lsp]` en la
/// configuración, y si no dice nada, el que Flint trae de fábrica para ese
/// lenguaje. Así agregar un servidor nuevo no toca el código.
fn lsp_command_for(cfg: &config::Config, lang: &highlight::Lang) -> Option<Vec<String>> {
    if let Some(cmd) = cfg.lsp_command(lang_id_str(lang)) {
        return Some(cmd.to_vec());
    }
    lang.lsp_command().map(|c| vec![c.to_string()])
}

fn main() -> io::Result<()> {
    let args = parse_args();
    let mut buffer = Buffer::open(args.path)?;

    // La configuración se lee antes que nada: de ella salen el perfil de
    // teclado, el tema y las opciones de edición. Las banderas de la línea
    // de comandos siguen ganando por encima, para poder probar algo distinto
    // sin editar el archivo.
    let config_path = args.config.clone().or_else(config::Config::default_path);
    let (cfg, config_warnings) = match &config_path {
        Some(p) if p.exists() => config::Config::load(p),
        _ => (config::Config::default(), Vec::new()),
    };
    if let Some(first) = config_warnings.first() {
        let extra = config_warnings.len() - 1;
        let cola = if extra > 0 {
            format!(" (+{extra} aviso(s) más)")
        } else {
            String::new()
        };
        buffer.ed.status = format!("{} — config: {first}{cola}", buffer.ed.status);
    }

    let profile_name = args
        .profile
        .clone()
        .or_else(|| cfg.profile.clone())
        .unwrap_or_else(|| "flint".to_string());
    let mut keymap = keymap::profile_by_name(&profile_name).unwrap_or_else(|| {
        buffer.ed.status = format!(
            "{} — perfil de teclado desconocido \"{profile_name}\", uso Flint",
            buffer.ed.status
        );
        keymap::flint_profile()
    });
    if keymap.name != "flint" {
        buffer.ed.status = format!("{} — perfil de teclado: {}", buffer.ed.status, keymap.name);
    }

    // Los remapeos se aplican sobre el perfil ya armado, así que la
    // configuración solo tiene que nombrar las teclas que quiere cambiar.
    for kb in &cfg.keys {
        if kb.modal {
            keymap::rebind_normal(&mut keymap, kb.chord, kb.action);
        } else {
            keymap::rebind_direct(&mut keymap, kb.chord, kb.action);
        }
    }
    if !cfg.keys.is_empty() {
        buffer.ed.status = format!("{} · {} tecla(s) remapeada(s)", buffer.ed.status, cfg.keys.len());
    }

    // `--theme <ruta>` explícito manda; si no, `~/.config/flint/theme.toml`
    // si existe; si no hay ninguno, la paleta ámbar de siempre. Si el archivo
    // sí existe, se recuerda su ruta y fecha de modificación para recargarlo
    // en caliente más adelante (ver `check_theme_reload`) — editar
    // `theme.toml` y guardar aplica los cambios sin reabrir Flint.
    let theme_path = args
        .theme
        .clone()
        .or_else(|| cfg.theme.clone())
        .or_else(theme::Theme::default_path);
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
    let opciones = resolve_options(&cfg, &theme, buffer.ed.filename.as_deref());
    apply_options(&mut buffer.ed, &opciones);

    if let Some(l) = &buffer.lang {
        buffer.ed.status = format!("{} — resaltado: {}", buffer.ed.status, l.label());
    }
    if let Some(respaldo) = buffer.ed.pending_backup() {
        buffer.ed.status = format!(
            "{} — ATENCIÓN: quedó un respaldo más nuevo que el archivo en {}",
            buffer.ed.status,
            respaldo.display()
        );
    }

    let plugins_dirs = plugins::default_dirs();
    let mut carga = plugins::PluginBridge::load(&plugins_dirs);
    let mut palette_entries = builtin_palette_entries();
    for (i, cmd) in carga.commands.iter().enumerate() {
        palette_entries.push(PaletteEntry {
            label: format!("{} (plugin)", cmd.label),
            kind: CommandKind::Plugin(i),
        });
    }
    // Las teclas que pidieron los plugins se resuelven recién acá, con la
    // lista completa de comandos ya armada: un plugin puede atar una tecla
    // antes de registrar el comando al que apunta, o apuntar al comando de
    // otro plugin que se cargue después.
    let plugin_keys = resolver_binds(&mut keymap, &carga.commands, &carga.binds, &mut carga.errors);
    if !carga.commands.is_empty() {
        buffer.ed.status = format!(
            "{} · {} comando(s) de plugin",
            buffer.ed.status,
            carga.commands.len()
        );
    }
    if !carga.lenguajes.is_empty() {
        buffer.detectar_lenguaje();
        buffer.ed.status = format!("{} · lenguajes de plugin: {}", buffer.ed.status, carga.lenguajes.join(", "));
    }
    if let Some(first_error) = carga.errors.first() {
        buffer.ed.status = format!("{} · error de plugin: {first_error}", buffer.ed.status);
    }
    let plugin_commands = carga.commands;
    let plugin_host = carga.bridge;

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
    let lsp_arrancando = setup_lsp(&mut terminal, &mut buffer.ed, lang.as_ref(), &theme, &cfg);

    let mut app = App {
        buffers: vec![buffer],
        active: 0,
        text_area: Rect::default(),
        tabs_area: None,
        lsp: None,
        lsp_arrancando,
        lsp_lang: None,
        lsp_incremental: false,
        lsp_definition: false,
        lsp_rename: false,
        lsp_hover: false,
        lsp_format: false,
        lsp_firma: false,
        firma: None,
        lsp_forzar_sync: false,
        last_was_select_line: false,
        keymap,
        pending_prefix: None,
        palette_entries,
        plugin_commands,
        plugins: Some(plugin_host),
        plugin_keys,
        theme,
        theme_path: watched_theme_path,
        theme_mtime,
        theme_next_check: Instant::now() + THEME_RELOAD_INTERVAL,
        clipboard_mode: cfg.clipboard,
        macro_recording: None,
        macro_last: Vec::new(),
        file_index: Vec::new(),
        search_files: Vec::new(),
        search_root: PathBuf::new(),
        search_partial: false,
        search_hits: Vec::new(),
        saltos: saltos::ListaSaltos::default(),
        hover_lines: Vec::new(),
        config: cfg,
        clipboard: arboard::Clipboard::new().ok(),
        preview_lines: Vec::new(),
        preview_key: None,
        en_evento: false,
    };
    emitir_evento(&mut app, plugins::Evento::Abrir);

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

/// Un servidor de lenguaje que ya se lanzó y todavía no contestó el
/// `initialize`. Vive aparte de `App::lsp` a propósito: hasta que conteste,
/// el protocolo no deja mandarle nada más, y así ningún pedido (autocompletar,
/// sincronizar) lo encuentra antes de tiempo.
struct ArranqueLsp {
    client: lsp::LspClient,
    id: u64,
    lang_id: &'static str,
    cmd: String,
    limite: Instant,
}

/// Cuánto se espera a que un servidor conteste el `initialize`. Mientras
/// tanto se edita normalmente; pasado esto se sigue sin LSP.
const ESPERA_ARRANQUE_LSP: Duration = Duration::from_secs(20);

/// Lanza el servidor de lenguaje para el archivo, si Flint sabe de uno, y le
/// manda el `initialize` sin esperar la respuesta: la atiende el bucle
/// principal (`atender_arranque_lsp`). Si el servidor no está instalado,
/// ofrece instalarlo (nunca en silencio: siempre se pregunta primero).
fn setup_lsp(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ed: &mut Editor,
    lang: Option<&highlight::Lang>,
    theme: &theme::Theme,
    cfg: &config::Config,
) -> Option<ArranqueLsp> {
    let lang = lang?;
    let partes = lsp_command_for(cfg, lang)?;
    let cmd = partes[0].as_str();
    let cmd_args = &partes[1..];
    let path = ed.filename.clone()?;
    let root = lsp::file_uri(path.parent().unwrap_or(Path::new(".")));

    let client = match lsp::LspClient::spawn(cmd, cmd_args) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !prompt_install(terminal, ed, cmd, theme) {
                ed.status = format!("Sin LSP para este archivo (falta {cmd})");
                return None;
            }
            match lsp::LspClient::spawn(cmd, cmd_args) {
                Ok(c) => c,
                Err(e2) => {
                    ed.status = format!("Sigue sin encontrarse {cmd}: {e2}");
                    return None;
                }
            }
        }
        Err(e) => {
            ed.status = format!("No se pudo iniciar {cmd}: {e}");
            return None;
        }
    };
    match lanzar_arranque(client, &root, lang_id_str(lang), cmd) {
        Ok(arranque) => {
            ed.status = format!("Iniciando {cmd}…");
            Some(arranque)
        }
        Err(e) => {
            ed.status = e;
            None
        }
    }
}

/// Manda el `initialize` y deja el servidor esperando su respuesta.
fn lanzar_arranque(
    mut client: lsp::LspClient,
    root: &str,
    lang_id: &'static str,
    cmd: &str,
) -> Result<ArranqueLsp, String> {
    let id = client
        .initialize(root)
        .map_err(|e| format!("No se pudo hablar con {cmd}: {e}"))?;
    Ok(ArranqueLsp {
        client,
        id,
        lang_id,
        cmd: cmd.to_string(),
        limite: Instant::now() + ESPERA_ARRANQUE_LSP,
    })
}

/// Mira si el servidor que está arrancando ya contestó. Cuando contesta, pasa
/// a ser `app.lsp` y se le abren todos los buffers de su lenguaje con el
/// texto que tengan ahora, lo escrito mientras arrancaba incluido. Si no
/// contesta a tiempo o se cae, se sigue sin LSP y la barra dice por qué.
fn atender_arranque_lsp(app: &mut App) {
    let Some(arranque) = app.lsp_arrancando.as_mut() else { return };
    let mut respuesta = None;
    while let Some(msg) = arranque.client.try_recv() {
        if msg.get("id").and_then(Value::as_u64) == Some(arranque.id) && msg.get("method").is_none() {
            respuesta = Some(msg);
            break;
        }
        // Antes de contestar, un servidor puede pedir cosas (una barra de
        // progreso, la configuración): se contestan igual que después.
        if let (Some(id), Some(metodo)) = (msg.get("id"), msg.get("method").and_then(Value::as_str)) {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            arranque.client.respond_default(id.clone(), metodo, &params);
        }
    }
    let fallo = match &respuesta {
        Some(msg) => msg
            .get("error")
            .map(|e| {
                let detalle = e.get("message").and_then(Value::as_str).unwrap_or("error sin detalle");
                format!("{} rechazó el arranque: {detalle}; sigo sin LSP", arranque.cmd)
            })
            .or_else(|| {
                msg.get("result")
                    .is_none_or(Value::is_null)
                    .then(|| format!("{} contestó el arranque vacío; sigo sin LSP", arranque.cmd))
            }),
        None if !arranque.client.is_alive() => Some(format!("{} se cerró al arrancar; sigo sin LSP", arranque.cmd)),
        None if Instant::now() >= arranque.limite => Some(format!(
            "{} no respondió en {} s; sigo sin LSP",
            arranque.cmd,
            ESPERA_ARRANQUE_LSP.as_secs()
        )),
        None => return,
    };
    let arranque = app.lsp_arrancando.take().expect("recién mirado");
    if let Some(motivo) = fallo {
        app.buffers[app.active].ed.status = motivo;
        return;
    }
    let result = respuesta.and_then(|m| m.get("result").cloned()).unwrap_or(Value::Null);
    let caps = Capacidades::de(&result);
    let ArranqueLsp { mut client, id, lang_id, cmd, .. } = arranque;
    client.pending.remove(&id);
    let _ = client.send_initialized();
    app.lsp = Some(client);
    app.lsp_lang = Some(lang_id);
    app.lsp_incremental = caps.incremental;
    app.lsp_definition = caps.definition;
    app.lsp_rename = caps.rename;
    app.lsp_hover = caps.hover;
    app.lsp_format = caps.format;
    app.lsp_firma = caps.firma;
    for idx in 0..app.buffers.len() {
        try_attach_lsp(app, idx);
    }
    app.buffers[app.active].ed.status = format!("{cmd} listo{}", resumen_de_capacidades(&caps));
}

/// Lo que se cuenta en la barra cuando un servidor queda listo.
fn resumen_de_capacidades(caps: &Capacidades) -> String {
    let sync_tag = if caps.incremental { " (sync incremental)" } else { " (sync completo)" };
    let sabe: Vec<&str> = [
        (caps.definition, "definición"),
        (caps.rename, "renombre"),
        (caps.hover, "hover"),
        (caps.format, "formato"),
        (caps.firma, "firmas"),
    ]
    .iter()
    .filter(|(si, _)| *si)
    .map(|(_, nombre)| *nombre)
    .collect();
    let extras = if sabe.is_empty() { String::new() } else { format!(" · {}", sabe.join(", ")) };
    format!("{sync_tag}{extras}")
}

/// Lo que el servidor dijo que sabe hacer, de lo que a Flint le importa.
#[derive(Clone, Copy, Default)]
struct Capacidades {
    /// `textDocumentSync.change == 2`: se le pueden mandar deltas en vez del
    /// documento entero.
    incremental: bool,
    definition: bool,
    rename: bool,
    hover: bool,
    format: bool,
    firma: bool,
}

impl Capacidades {
    fn de(result: &Value) -> Capacidades {
        let caps = result.get("capabilities");
        // Cada capacidad puede venir como `true` o como un objeto con
        // opciones; las dos formas significan que la soporta. `false` o
        // ausente significan que no.
        let tiene = |nombre: &str| {
            caps.and_then(|c| c.get(nombre))
                .is_some_and(|v| v.as_bool() != Some(false) && !v.is_null())
        };
        Capacidades {
            incremental: supports_incremental_sync(result),
            definition: tiene("definitionProvider"),
            rename: tiene("renameProvider"),
            hover: tiene("hoverProvider"),
            format: tiene("documentFormattingProvider"),
            firma: tiene("signatureHelpProvider"),
        }
    }
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

/// Diálogo mínimo, autónomo, que se dibuja antes de que arranque el bucle
/// principal: pregunta si instalar el servidor de lenguaje y, si el usuario
/// confirma, lo instala de verdad (bloqueante, una sola vez).
fn prompt_install(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ed: &mut Editor,
    cmd: &str,
    theme: &theme::Theme,
) -> bool {
    // El único servidor que Flint sabe instalar es rust-analyzer, que viene
    // como componente de rustup. Para cualquier otro se avisa y se sigue sin
    // LSP: adivinar el gestor de paquetes de la máquina sería peor que no
    // ofrecer nada, y este diálogo bloquea el arranque.
    if cmd != "rust-analyzer" {
        ed.status = format!("Falta el servidor \"{cmd}\"; instalalo y volvé a abrir el archivo");
        return false;
    }
    let install_cmd = "rustup";
    let install_args = ["component", "add", "rust-analyzer"];
    ed.status = format!(
        "{cmd} no está instalado. ¿Instalarlo con \"{install_cmd} {}\"? (s = sí, n = no): ",
        install_args.join(" ")
    );
    loop {
        let _ = terminal.draw(|f| {
            ui::draw(f, ed, &ui::FrameData::default(), theme);
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
        ui::draw(f, ed, &ui::FrameData::default(), theme);
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
            // Sin margen de espera: el resaltado reusa el árbol de la pasada
            // anterior, así que una tecla cuesta lo que cuesta reanalizar lo
            // que esa tecla tocó y no el archivo entero. Antes había que
            // esperar a que la ráfaga de tipeo terminara para no gastar un
            // recorrido completo por letra.
            let buf = &mut app.buffers[app.active];
            highlight::refresh(&mut buf.ed, buf.highlighter.as_mut());
        }
        poll_lsp(app);
        sync_lsp_if_needed(app);
        avisar_guardados(app);
        check_theme_reload(app);
        write_backups(app);

        let palette_labels: Vec<String> = match &app.buffers[app.active].ed.mode {
            Mode::Palette { query, .. } => filtered_palette_indices(app, query)
                .into_iter()
                .filter_map(|i| app.palette_entries.get(i).map(|e| e.label.clone()))
                .collect(),
            Mode::FilePicker { query, .. } => filtered_file_indices(app, query)
                .into_iter()
                .filter_map(|i| app.file_index.get(i).cloned())
                .collect(),
            Mode::ProjectSearch { .. } => app
                .search_hits
                .iter()
                .map(|c| {
                    let ruta = app.search_files.get(c.archivo).map_or("", |a| a.ruta.as_str());
                    format!("{ruta}:{}: {}", c.linea + 1, c.texto)
                })
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
        cerrar_firma_si_corresponde(app);
        let tab_labels: Vec<String> = app.buffers.iter().map(Buffer::tab_label).collect();
        let ancho_total = terminal.size().map(|s| s.width).unwrap_or(80);
        refresh_preview(app, ancho_total);
        let preview_lines = if app.buffers[app.active].ed.preview {
            Some(&app.preview_lines)
        } else {
            None
        };
        let firma_linea = app.firma.as_ref().map(|f| linea_de_firma(&f.firma, &app.theme));
        terminal.draw(|f| {
            let datos = ui::FrameData {
                palette_matches: &palette_labels,
                completion_matches: &completion_view,
                tab_labels: &tab_labels,
                active_tab: app.active,
                preview: preview_lines.map(Vec::as_slice),
                hover: &app.hover_lines,
                firma: firma_linea.as_ref(),
            };
            let areas = ui::draw(f, &mut app.buffers[app.active].ed, &datos, &app.theme);
            app.text_area = areas.text_area;
            app.tabs_area = areas.tabs_area;
        })?;

        if process_pending_quit(app) {
            return Ok(());
        }

        // Con el primer resaltado corriendo en otro hilo, se vuelve a mirar
        // seguido para pintar apenas llegue; si no, cada 200 ms alcanza.
        let resaltando = app.buffers[app.active].highlighter.as_ref().is_some_and(|h| h.resaltando());
        let espera = Duration::from_millis(if resaltando || app.lsp_arrancando.is_some() { 20 } else { 200 });
        if event::poll(espera)? {
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

/// Deja en disco una copia de lo que cada buffer tiene sin guardar. Cada
/// `Editor` decide si le toca (mira si está sucio y cuánto pasó desde el
/// anterior), así que llamarlo en cada vuelta del bucle no cuesta nada.
fn write_backups(app: &mut App) {
    for buf in app.buffers.iter_mut() {
        buf.ed.write_backup_if_due(BACKUP_INTERVAL);
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
    // Cerrar es una decisión, no una caída: el respaldo se va con el buffer.
    // Si no, la próxima vez que se abra el archivo avisaría de un trabajo que
    // alguien ya decidió tirar.
    app.buffers[app.active].ed.discard_backup();
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
    atender_arranque_lsp(app);
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
        match pending {
            Some(lsp::Pending::Completion { buffer, trigger, prefix }) => {
                apply_completion(app, buffer, trigger, prefix, msg.get("result"));
            }
            Some(lsp::Pending::Definition { buffer, desde, palabra }) => {
                aplicar_definicion(app, buffer, desde, &palabra, msg.get("result"));
            }
            Some(lsp::Pending::Hover { buffer, desde, palabra }) => {
                aplicar_hover(app, buffer, desde, &palabra, msg.get("result"));
            }
            Some(lsp::Pending::Firma { buffer, linea }) => {
                aplicar_firma(app, buffer, linea, msg.get("result"));
            }
            Some(lsp::Pending::Rename { nombre }) => {
                aplicar_renombre(app, &nombre, msg.get("result"), msg.get("error"));
            }
            _ => {}
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
    let mut items: Vec<editor::CompletionEntry> = items_val
        .unwrap_or_default()
        .iter()
        .filter_map(completion_entry_from)
        .take(50)
        .collect();
    for it in &mut items {
        it.reemplazo = it.lsp_range.and_then(|r| reemplazo_de(&target.ed, trigger, &prefix, r));
    }

    if items.is_empty() {
        // El servidor no tenía nada que ofrecer acá: mejor las palabras del
        // propio texto que un "sin sugerencias" y el popup sin abrir.
        complete_from_buffer_in(&mut target.ed);
        return;
    }
    target.ed.mode = Mode::Completion {
        items,
        selected: 0,
        trigger: editor::Position { line: trigger.0, col: trigger.1 },
        prefix,
    };
}

// ---------- ir a la definición y renombrar ----------

/// Una posición de LSP: línea desde 0 y columna en unidades UTF-16.
fn pos_lsp(v: &Value) -> Option<(usize, usize)> {
    Some((
        v.get("line").and_then(Value::as_u64)? as usize,
        v.get("character").and_then(Value::as_u64)? as usize,
    ))
}

/// La ruta de un `file://…`, deshaciendo el `%XX` del camino.
fn ruta_de_uri(uri: &str) -> Option<PathBuf> {
    let resto = uri.strip_prefix("file://")?;
    // Se descarta el "authority" (lo que va entre // y la primera /) porque
    // para archivos locales siempre está vacío o es "localhost".
    let resto = match resto.find('/') {
        Some(0) => resto,
        Some(i) => &resto[i..],
        None => return None,
    };
    let bytes = resto.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

/// Saca de la respuesta a `textDocument/definition` el primer destino y
/// cuántos había. El protocolo admite tres formas: un `Location`, una lista
/// de `Location`, o una lista de `LocationLink` (que trae el rango del
/// nombre además del del cuerpo entero — se prefiere el del nombre, que es
/// donde uno quiere que caiga el cursor).
fn primer_destino(result: &Value) -> Option<(String, (usize, usize), usize)> {
    let lista: Vec<&Value> = match result {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![result],
        _ => return None,
    };
    let total = lista.len();
    let v = lista.first()?;
    if let Some(uri) = v.get("targetUri").and_then(Value::as_str) {
        let rango = v
            .get("targetSelectionRange")
            .or_else(|| v.get("targetRange"))?;
        let inicio = pos_lsp(rango.get("start")?)?;
        return Some((uri.to_string(), inicio, total));
    }
    let uri = v.get("uri").and_then(Value::as_str)?;
    let inicio = pos_lsp(v.get("range")?.get("start")?)?;
    Some((uri.to_string(), inicio, total))
}

fn aplicar_definicion(
    app: &mut App,
    buffer: usize,
    desde: (usize, usize),
    palabra: &str,
    result: Option<&Value>,
) {
    let destino = result.and_then(primer_destino);
    let Some((uri, (linea, col_utf16), total)) = destino else {
        app.buffers[app.active].ed.status =
            format!("No encontré dónde se define \"{palabra}\"");
        return;
    };

    // El mismo archivo o uno abierto en otra pestaña: se salta ahí. Si no,
    // se abre en un buffer nuevo.
    let idx = match app.buffers.iter().position(|b| b.doc_uri.as_deref() == Some(uri.as_str())) {
        Some(i) => Some(i),
        None => match ruta_de_uri(&uri) {
            Some(ruta) => {
                let antes = app.buffers.len();
                open_file_into_new_buffer(app, ruta.display().to_string());
                (app.buffers.len() > antes).then(|| app.buffers.len() - 1)
            }
            None => None,
        },
    };
    let Some(idx) = idx else {
        app.buffers[app.active].ed.status = format!("No pude abrir {uri}");
        return;
    };

    if let Some(desde) = lugar_de(app, buffer, desde) {
        app.saltos.registrar(desde);
    }
    app.active = idx;
    let ed = &mut app.buffers[idx].ed;
    let linea = linea.min(ed.line_count().saturating_sub(1));
    let col = ed.utf16_col_to_char(linea, col_utf16);
    ed.goto_line(linea);
    ed.cursor.col = col.min(ed.line_char_len(linea));
    // De dónde se venía, para poder contarlo: no hay lista de saltos
    // todavía, pero decir la línea de origen ya ahorra tener que acordarse.
    let volver = if buffer == idx && desde.0 != linea {
        format!(" (venías de la línea {})", desde.0 + 1)
    } else {
        String::new()
    };
    let otras = if total > 1 {
        format!(", {} definiciones en total", total)
    } else {
        String::new()
    };
    ed.status = format!("Definición de \"{palabra}\" en la línea {}{otras}{volver}", linea + 1);
}

/// Los cambios de un `WorkspaceEdit`, agrupados por URI. El protocolo admite
/// dos formas: `changes`, un objeto de URI a lista de ediciones, y
/// `documentChanges`, una lista que además lleva la versión del documento (y
/// que puede traer operaciones de archivo — crear, borrar, renombrar — que
/// Flint no aplica).
fn cambios_de_workspace_edit(edit: &Value) -> Result<Vec<(String, Vec<Value>)>, String> {
    let mut out: Vec<(String, Vec<Value>)> = Vec::new();
    if let Some(cambios) = edit.get("changes").and_then(Value::as_object) {
        for (uri, ediciones) in cambios {
            let Some(lista) = ediciones.as_array() else { continue };
            out.push((uri.clone(), lista.clone()));
        }
    }
    if let Some(docs) = edit.get("documentChanges").and_then(Value::as_array) {
        for doc in docs {
            if doc.get("kind").is_some() {
                // create/rename/delete de archivos. Aplicar la mitad de un
                // renombre sería peor que no aplicar nada, así que se corta.
                return Err("el servidor pidió crear o borrar archivos, que Flint no hace".to_string());
            }
            let Some(uri) = doc
                .get("textDocument")
                .and_then(|t| t.get("uri"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(lista) = doc.get("edits").and_then(Value::as_array) else {
                continue;
            };
            out.push((uri.to_string(), lista.clone()));
        }
    }
    Ok(out)
}

/// Cuántos archivos puede tocar un renombre antes de que Flint se plante.
/// No es un límite técnico: es que abrir ciento cincuenta buffers sin
/// guardar no es una operación que alguien pueda revisar.
const MAX_ARCHIVOS_RENOMBRE: usize = 50;

fn aplicar_renombre(app: &mut App, nombre: &str, result: Option<&Value>, error: Option<&Value>) {
    if let Some(e) = error {
        let msg = e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("el servidor rechazó el renombre");
        app.buffers[app.active].ed.status = format!("No se pudo renombrar: {msg}");
        return;
    }
    let Some(edit) = result.filter(|v| !v.is_null()) else {
        app.buffers[app.active].ed.status =
            "El servidor no devolvió ningún cambio; el nombre no se tocó".to_string();
        return;
    };
    let cambios = match cambios_de_workspace_edit(edit) {
        Ok(c) => c,
        Err(e) => {
            app.buffers[app.active].ed.status = format!("No se pudo renombrar: {e}");
            return;
        }
    };
    if cambios.is_empty() {
        app.buffers[app.active].ed.status = "El servidor no devolvió ningún cambio".to_string();
        return;
    }
    if cambios.len() > MAX_ARCHIVOS_RENOMBRE {
        app.buffers[app.active].ed.status = format!(
            "El renombre toca {} archivos, más del tope de {MAX_ARCHIVOS_RENOMBRE}; no lo apliqué",
            cambios.len()
        );
        return;
    }

    let volver_a = app.active;
    let mut tocados = 0usize;
    let mut abiertos = 0usize;
    let mut fallados: Vec<String> = Vec::new();

    for (uri, ediciones) in cambios {
        let idx = match app.buffers.iter().position(|b| b.doc_uri.as_deref() == Some(uri.as_str())) {
            Some(i) => Some(i),
            None => match ruta_de_uri(&uri) {
                Some(ruta) if ruta.exists() => {
                    let antes = app.buffers.len();
                    open_file_into_new_buffer(app, ruta.display().to_string());
                    if app.buffers.len() > antes {
                        abiertos += 1;
                        Some(app.buffers.len() - 1)
                    } else {
                        None
                    }
                }
                _ => None,
            },
        };
        let Some(idx) = idx else {
            fallados.push(uri);
            continue;
        };
        if aplicar_ediciones(&mut app.buffers[idx].ed, &ediciones) {
            tocados += 1;
        } else {
            fallados.push(uri);
        }
    }

    app.active = volver_a.min(app.buffers.len().saturating_sub(1));
    let sin_guardar = if abiertos > 0 {
        format!(", {abiertos} sin guardar")
    } else {
        String::new()
    };
    let con_error = if fallados.is_empty() {
        String::new()
    } else {
        format!(" · {} sin poder tocar", fallados.len())
    };
    app.buffers[app.active].ed.status = format!(
        "Renombrado a \"{nombre}\" en {tocados} archivo(s){sin_guardar}{con_error}"
    );
}

/// Aplica una lista de `TextEdit` a un buffer, de atrás para adelante.
///
/// El orden importa: el protocolo garantiza que los rangos de una misma
/// lista no se superponen, pero sí están todos expresados contra el
/// documento original. Aplicarlos de adelante para atrás correría los de
/// más abajo y cada uno caería un poco más desalineado que el anterior.
fn aplicar_ediciones(ed: &mut Editor, ediciones: &[Value]) -> bool {
    /// Un reemplazo tal como lo manda el servidor: desde, hasta y con qué,
    /// con las posiciones todavía en coordenadas de LSP.
    type Reemplazo = ((usize, usize), (usize, usize), String);

    let mut rangos: Vec<Reemplazo> = Vec::new();
    for e in ediciones {
        let Some(rango) = e.get("range") else { return false };
        let (Some(inicio), Some(fin)) = (
            rango.get("start").and_then(pos_lsp),
            rango.get("end").and_then(pos_lsp),
        ) else {
            return false;
        };
        let texto = e.get("newText").and_then(Value::as_str).unwrap_or("");
        rangos.push((inicio, fin, texto.to_string()));
    }
    if rangos.is_empty() {
        return false;
    }
    let cambios: Vec<(editor::Position, editor::Position, String)> = rangos
        .into_iter()
        .map(|((l0, c0), (l1, c1), texto)| {
            let l0 = l0.min(ed.line_count().saturating_sub(1));
            let l1 = l1.min(ed.line_count().saturating_sub(1));
            (
                editor::Position { line: l0, col: ed.utf16_col_to_char(l0, c0) },
                editor::Position { line: l1, col: ed.utf16_col_to_char(l1, c1) },
                texto,
            )
        })
        .collect();
    ed.replace_ranges(&cambios) > 0
}


/// Prepara una petición que tiene que salir *ahora*, no cuando venza el
/// margen de espera del LSP.
///
/// Devuelve la URI del documento y la posición del cursor en las unidades que
/// pide el protocolo, o `None` con el motivo ya puesto en la barra de estado.
/// Antes de devolverla fuerza la sincronización: si el servidor tiene una
/// versión vieja del archivo, "ir a la definición" salta a donde estaba el
/// símbolo hace medio segundo, que es peor que no saltar.
fn preparar_peticion(app: &mut App, que: &str) -> Option<(String, usize, usize)> {
    if app.lsp.is_none() {
        app.buffers[app.active].ed.status =
            format!("{que} necesita un servidor de lenguaje, y este archivo no tiene");
        return None;
    }
    let Some(uri) = app.buffers[app.active].doc_uri.clone() else {
        app.buffers[app.active].ed.status =
            format!("{que} necesita que el archivo esté abierto en el servidor");
        return None;
    };
    sync_lsp_ahora(app);
    let ed = &app.buffers[app.active].ed;
    let linea = ed.cursor.line;
    let col = ed.char_col_to_utf16(linea, ed.cursor.col);
    Some((uri, linea, col))
}

/// Igual que `sync_lsp_if_needed` pero sin esperar el margen: se usa justo
/// antes de una petición que depende de que el servidor tenga el texto de
/// ahora.
fn sync_lsp_ahora(app: &mut App) {
    let anterior = app.lsp_forzar_sync;
    app.lsp_forzar_sync = true;
    sync_lsp_if_needed(app);
    app.lsp_forzar_sync = anterior;
}

fn goto_definition(app: &mut App) {
    if !app.lsp_definition {
        app.buffers[app.active].ed.status =
            "El servidor de lenguaje no sabe ir a la definición".to_string();
        return;
    }
    let palabra = app.buffers[app.active].ed.word_under_cursor();
    if palabra.is_empty() {
        app.buffers[app.active].ed.status = "El cursor no está sobre ningún nombre".to_string();
        return;
    }
    let Some((uri, linea, col)) = preparar_peticion(app, "Ir a la definición") else {
        return;
    };
    let desde = {
        let ed = &app.buffers[app.active].ed;
        (ed.cursor.line, ed.cursor.col)
    };
    let activo = app.active;
    let enviado = app
        .lsp
        .as_mut()
        .map(|c| c.request_definition(&uri, linea, col, activo, desde, palabra.clone()))
        .transpose();
    match enviado {
        Ok(_) => {
            app.buffers[app.active].ed.status = format!("Buscando dónde se define \"{palabra}\"…")
        }
        Err(e) => app.buffers[app.active].ed.status = format!("No pude preguntarle al servidor: {e}"),
    }
}

fn pedir_hover(app: &mut App) {
    if app.lsp.is_some() && !app.lsp_hover {
        app.buffers[app.active].ed.status =
            "El servidor de lenguaje no sabe mostrar información (hover)".to_string();
        return;
    }
    let Some((uri, linea, col)) = preparar_peticion(app, "La información de hover") else {
        return;
    };
    let ed = &app.buffers[app.active].ed;
    let palabra = ed.word_under_cursor();
    let desde = (ed.cursor.line, ed.cursor.col);
    let activo = app.active;
    let enviado = app
        .lsp
        .as_mut()
        .map(|c| c.request_hover(&uri, linea, col, activo, desde, palabra.clone()))
        .transpose();
    if let Err(e) = enviado {
        app.buffers[app.active].ed.status = format!("No pude preguntarle al servidor: {e}");
    }
}

/// La ayuda de firmas que está a la vista, y de qué buffer y línea habla.
struct FirmaAbierta {
    buffer: usize,
    linea: usize,
    firma: Firma,
}

/// Lo que se muestra de una respuesta de `signatureHelp`: la firma activa,
/// dónde está en ella el parámetro que se está escribiendo (en caracteres),
/// y cuál de cuántas firmas es (una función puede tener varias).
#[derive(Debug, PartialEq)]
struct Firma {
    etiqueta: String,
    parametro: Option<(usize, usize)>,
    indice: usize,
    total: usize,
}

/// `automatico`: la pidió una tecla y no el usuario. Ahí, si no hay servidor
/// o no sabe de firmas, no se dice nada: sería un aviso en cada paréntesis.
fn pedir_firma(app: &mut App, automatico: bool) {
    let sin_soporte = app.lsp.is_none() || !app.lsp_firma || app.buffers[app.active].doc_uri.is_none();
    if sin_soporte {
        app.firma = None;
        if !automatico {
            app.buffers[app.active].ed.status = if app.lsp.is_none() {
                "La ayuda de firmas necesita un servidor de lenguaje, y este archivo no tiene".to_string()
            } else {
                "El servidor de lenguaje no sabe mostrar firmas".to_string()
            };
        }
        return;
    }
    let Some((uri, linea, col)) = preparar_peticion(app, "La ayuda de firmas") else {
        return;
    };
    let activo = app.active;
    let enviado = app
        .lsp
        .as_mut()
        .map(|c| c.request_signature_help(&uri, linea, col, activo))
        .transpose();
    if let Err(e) = enviado {
        app.buffers[app.active].ed.status = format!("No pude preguntarle al servidor: {e}");
    }
}

fn aplicar_firma(app: &mut App, buffer: usize, linea: usize, result: Option<&Value>) {
    let ed = &app.buffers[app.active].ed;
    if app.active != buffer || ed.cursor.line != linea {
        return;
    }
    // Sin firmas: el cursor ya no está en una llamada. Se cierra.
    app.firma = result
        .and_then(firma_de)
        .map(|firma| FirmaAbierta { buffer, linea, firma });
}

/// La ayuda habla de la línea donde se pidió: si el cursor se fue, o se
/// cambió de pestaña, o se abrió algo que no es escribir, se cierra.
fn cerrar_firma_si_corresponde(app: &mut App) {
    let Some(f) = &app.firma else { return };
    let ed = &app.buffers[app.active].ed;
    let sigue = f.buffer == app.active
        && f.linea == ed.cursor.line
        && matches!(ed.mode, Mode::Editing | Mode::Completion { .. })
        && !ed.preview;
    if !sigue {
        app.firma = None;
    }
}

fn firma_de(result: &Value) -> Option<Firma> {
    let firmas = result.get("signatures")?.as_array()?;
    if firmas.is_empty() {
        return None;
    }
    let indice = result.get("activeSignature").and_then(Value::as_u64).unwrap_or(0) as usize;
    let indice = if indice < firmas.len() { indice } else { 0 };
    let firma = &firmas[indice];
    let etiqueta = firma.get("label")?.as_str()?.to_string();
    // El de la firma gana sobre el general. Ausente vale 0; `null` quiere
    // decir que no hay parámetro activo.
    let activo = match firma.get("activeParameter").or_else(|| result.get("activeParameter")) {
        Some(v) => v.as_u64(),
        None => Some(0),
    };
    let parametro = activo.and_then(|i| {
        let parametros = firma.get("parameters")?.as_array()?;
        // Fuera de rango, el spec dice que vale el primero.
        let p = parametros.get(i as usize).or_else(|| parametros.first())?;
        rango_de_parametro(&etiqueta, p.get("label")?)
    });
    Some(Firma { etiqueta, parametro, indice, total: firmas.len() })
}

/// Dónde está un parámetro dentro de la firma, en caracteres. El servidor lo
/// dice de dos formas: con el texto del parámetro (se busca después del
/// primer paréntesis, para no confundirlo con el nombre de la función) o con
/// un par de posiciones en UTF-16.
fn rango_de_parametro(etiqueta: &str, label: &Value) -> Option<(usize, usize)> {
    match label {
        Value::String(texto) if !texto.is_empty() => {
            let desde = etiqueta.find('(').map_or(0, |i| i + 1);
            let byte = etiqueta[desde..].find(texto.as_str())? + desde;
            let inicio = etiqueta[..byte].chars().count();
            Some((inicio, inicio + texto.chars().count()))
        }
        Value::Array(par) => {
            let a = par.first()?.as_u64()? as usize;
            let b = par.get(1)?.as_u64()? as usize;
            let (mut inicio, mut fin) = (None, None);
            let mut unidades = 0;
            for (i, c) in etiqueta.chars().chain(std::iter::once('\0')).enumerate() {
                if unidades == a {
                    inicio = Some(i);
                }
                if unidades == b {
                    fin = Some(i);
                }
                unidades += c.len_utf16();
            }
            Some((inicio?, fin?)).filter(|(i, f)| i < f)
        }
        _ => None,
    }
}

fn linea_de_firma(firma: &Firma, theme: &theme::Theme) -> ratatui::text::Line<'static> {
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Span;
    let normal = Style::default().fg(theme.text_fg);
    let chars: Vec<char> = firma.etiqueta.chars().collect();
    let (a, b) = firma.parametro.unwrap_or((chars.len(), chars.len()));
    let tramo = |desde: usize, hasta: usize| chars[desde..hasta].iter().collect::<String>();
    let mut spans = vec![
        Span::styled(format!(" {}", tramo(0, a)), normal),
        Span::styled(tramo(a, b), normal.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)),
        Span::styled(tramo(b, chars.len()), normal),
    ];
    if firma.total > 1 {
        spans.push(Span::styled(
            format!("  ({}/{})", firma.indice + 1, firma.total),
            Style::default().fg(theme.dim),
        ));
    }
    spans.push(Span::raw(" "));
    ratatui::text::Line::from(spans)
}

/// El texto de una respuesta de hover y si es Markdown. El protocolo admite
/// tres formas para `contents`: `MarkupContent` (`{kind, value}`, la
/// actual), y las dos viejas, `MarkedString` suelto (un string Markdown o
/// `{language, value}`, que es un bloque de código) o una lista de ellos.
fn contenido_de_hover(result: &Value) -> Option<(String, bool)> {
    fn marked(v: &Value) -> Option<String> {
        match v {
            Value::String(s) => Some(s.clone()),
            Value::Object(o) => {
                let valor = o.get("value")?.as_str()?;
                let lenguaje = o.get("language").and_then(Value::as_str).unwrap_or("");
                Some(format!("```{lenguaje}\n{valor}\n```"))
            }
            _ => None,
        }
    }
    let contents = result.get("contents")?;
    let (texto, markdown) = match contents {
        Value::Object(o) if o.contains_key("kind") => (
            o.get("value")?.as_str()?.to_string(),
            o.get("kind").and_then(Value::as_str) == Some("markdown"),
        ),
        Value::Array(lista) => {
            let partes: Vec<String> = lista.iter().filter_map(marked).collect();
            (partes.join("\n\n"), true)
        }
        otro => (marked(otro)?, true),
    };
    let texto = texto.trim().to_string();
    (!texto.is_empty()).then_some((texto, markdown))
}

/// Parte texto plano en renglones de hasta `ancho` columnas. El Markdown lo
/// reparte `markdown::render`; esto es para los servidores que mandan texto
/// sin formato.
fn envolver_texto_plano(texto: &str, ancho: usize) -> Vec<ratatui::text::Line<'static>> {
    let mut lineas = Vec::new();
    for renglon in texto.lines() {
        let mut actual = String::new();
        let mut columnas = 0usize;
        for c in renglon.chars() {
            let w = editor::char_display_width(c, columnas, 4);
            if columnas + w > ancho && !actual.is_empty() {
                lineas.push(ratatui::text::Line::from(std::mem::take(&mut actual)));
                columnas = 0;
            }
            if c == '\t' {
                actual.push_str(&" ".repeat(w));
            } else {
                actual.push(c);
            }
            columnas += w;
        }
        lineas.push(ratatui::text::Line::from(actual));
    }
    lineas
}

fn aplicar_hover(app: &mut App, buffer: usize, desde: (usize, usize), palabra: &str, result: Option<&Value>) {
    // Si el cursor se movió o se cambió de pestaña mientras el servidor
    // pensaba, la respuesta habla de algo que ya no está a la vista.
    let ed = &app.buffers[app.active].ed;
    if app.active != buffer || (ed.cursor.line, ed.cursor.col) != desde || !matches!(ed.mode, Mode::Editing) {
        return;
    }
    let Some((texto, markdown)) = result.and_then(contenido_de_hover) else {
        app.buffers[app.active].ed.status = if palabra.is_empty() {
            "El servidor no tiene información de esto".to_string()
        } else {
            format!("El servidor no tiene información de \"{palabra}\"")
        };
        return;
    };
    let ancho = (app.text_area.width as usize).saturating_sub(4).clamp(20, 90);
    let mut lineas = if markdown {
        markdown::render(&texto, ancho, &app.theme, &|nombre| markdown::resaltador_para(nombre))
    } else {
        envolver_texto_plano(&texto, ancho)
    };
    // El renderizado rellena con espacios hasta el ancho disponible, que en
    // la vista previa ocupa la pantalla entera; acá la ventana tiene que
    // medir lo que mide el texto.
    for linea in lineas.iter_mut() {
        while let Some(ultimo) = linea.spans.last_mut() {
            let recortado = ultimo.content.trim_end().to_string();
            if recortado.is_empty() {
                linea.spans.pop();
            } else {
                ultimo.content = recortado.into();
                break;
            }
        }
    }
    // Y deja renglones vacíos entre bloques; en las puntas solo agrandan la
    // ventana.
    while lineas.last().is_some_and(|l| l.width() == 0) {
        lineas.pop();
    }
    while lineas.first().is_some_and(|l| l.width() == 0) {
        lineas.remove(0);
    }
    app.hover_lines = lineas;
    app.buffers[app.active].ed.mode = Mode::Hover { offset: 0 };
}

/// Con el hover abierto, las flechas lo recorren y Esc lo cierra. Cualquier
/// otra tecla lo cierra y además hace lo suyo: no hace falta cerrarlo para
/// seguir escribiendo.
fn handle_hover_key(app: &mut App, key: KeyEvent, page_size: usize) {
    let Mode::Hover { offset } = app.buffers[app.active].ed.mode else { return };
    // El tope fino lo pone el dibujado, que sabe cuántas líneas entraron.
    let maximo = app.hover_lines.len().saturating_sub(1);
    let nuevo = match key.code {
        KeyCode::Up => Some(offset.saturating_sub(1)),
        KeyCode::Down => Some((offset + 1).min(maximo)),
        KeyCode::PageUp => Some(offset.saturating_sub(ui::HOVER_ALTO)),
        KeyCode::PageDown => Some((offset + ui::HOVER_ALTO).min(maximo)),
        _ => None,
    };
    if let Some(offset) = nuevo {
        app.buffers[app.active].ed.mode = Mode::Hover { offset };
        return;
    }
    app.buffers[app.active].ed.mode = Mode::Editing;
    app.hover_lines.clear();
    if key.code != KeyCode::Esc {
        handle_key(app, key, page_size);
    }
}

/// Dónde está el cursor del buffer `idx`, como lugar de la lista de saltos.
/// La ruta va canónica para que el mismo archivo abierto por dos caminos
/// (`src/main.rs` y `./src/main.rs`) cuente como uno.
fn lugar_de(app: &App, idx: usize, (linea, col): (usize, usize)) -> Option<saltos::Lugar> {
    let ruta = app.buffers.get(idx)?.ed.filename.as_ref()?;
    let ruta = std::fs::canonicalize(ruta).unwrap_or_else(|_| ruta.clone());
    Some(saltos::Lugar { ruta, linea, col })
}

fn lugar_actual(app: &App) -> Option<saltos::Lugar> {
    let ed = &app.buffers[app.active].ed;
    lugar_de(app, app.active, (ed.cursor.line, ed.cursor.col))
}

/// Anota dónde está el cursor antes de un salto, para poder volver.
fn registrar_salto(app: &mut App) {
    if let Some(lugar) = lugar_actual(app) {
        app.saltos.registrar(lugar);
    }
}

fn saltar_en_la_lista(app: &mut App, atras: bool) {
    let actual = lugar_actual(app);
    let destino = if atras { app.saltos.volver(actual) } else { app.saltos.avanzar(actual) };
    let Some(destino) = destino else {
        app.buffers[app.active].ed.status =
            if atras { "No hay a dónde volver" } else { "No hay a dónde avanzar" }.to_string();
        return;
    };
    let abierto = app.buffers.iter().position(|b| {
        b.ed.filename
            .as_ref()
            .map(|f| std::fs::canonicalize(f).unwrap_or_else(|_| f.clone()))
            .as_ref()
            == Some(&destino.ruta)
    });
    match abierto {
        Some(i) => app.active = i,
        None => {
            // La pestaña se cerró desde entonces: se vuelve a abrir.
            let antes = app.buffers.len();
            open_file_into_new_buffer(app, destino.ruta.to_string_lossy().to_string());
            if app.buffers.len() == antes {
                return;
            }
        }
    }
    let (atras_n, adelante_n) = (app.saltos.cuantos_atras(), app.saltos.cuantos_adelante());
    let ed = &mut app.buffers[app.active].ed;
    ed.goto_line(destino.linea);
    ed.cursor.col = destino.col.min(ed.line_char_len(ed.cursor.line));
    ed.status = format!(
        "Línea {} · {atras_n} atrás, {adelante_n} adelante",
        ed.cursor.line + 1
    );
}

fn rename_prompt(app: &mut App) {
    if !app.lsp_rename {
        app.buffers[app.active].ed.status =
            "El servidor de lenguaje no sabe renombrar".to_string();
        return;
    }
    let palabra = app.buffers[app.active].ed.word_under_cursor();
    if palabra.is_empty() {
        app.buffers[app.active].ed.status = "El cursor no está sobre ningún nombre".to_string();
        return;
    }
    app.buffers[app.active].ed.mode = Mode::Prompt {
        label: format!("Renombrar \"{palabra}\" a: "),
        // Arranca con el nombre actual escrito: renombrar casi siempre es
        // retocar lo que ya está, no escribirlo de cero.
        buffer: palabra.clone(),
        kind: PromptKind::Rename { palabra },
    };
}

fn pedir_renombre(app: &mut App, palabra: String, nuevo: String) {
    let nuevo = nuevo.trim().to_string();
    if nuevo.is_empty() || nuevo == palabra {
        app.buffers[app.active].ed.status = "Renombre cancelado".to_string();
        return;
    }
    let Some((uri, linea, col)) = preparar_peticion(app, "Renombrar") else {
        return;
    };
    let enviado = app
        .lsp
        .as_mut()
        .map(|c| c.request_rename(&uri, linea, col, &nuevo))
        .transpose();
    match enviado {
        Ok(_) => {
            app.buffers[app.active].ed.status =
                format!("Renombrando \"{palabra}\" a \"{nuevo}\"…")
        }
        Err(e) => app.buffers[app.active].ed.status = format!("No pude preguntarle al servidor: {e}"),
    }
}

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
        let quiet_enough = app.lsp_forzar_sync
            || buf
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
/// Rehace la vista previa si cambió el buffer activo, su contenido o el
/// ancho de la pantalla. Si no, deja la que ya estaba.
fn refresh_preview(app: &mut App, ancho_total: u16) {
    if !app.buffers[app.active].ed.preview {
        app.preview_key = None;
        app.preview_lines.clear();
        return;
    }
    let gutter = app.buffers[app.active].ed.gutter_width();
    let ancho = ancho_total.saturating_sub(gutter + 2);
    let clave = (app.active, app.buffers[app.active].ed.content_version, ancho);
    if app.preview_key == Some(clave) {
        return;
    }
    let fuente = app.buffers[app.active].ed.rope.to_string();
    app.preview_lines = markdown::render(&fuente, ancho as usize, &app.theme, &|nombre| {
        markdown::resaltador_para(nombre)
    });
    app.preview_key = Some(clave);
}

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
    // El ancho de tabulación puede venir del tema, así que recargarlo obliga
    // a volver a resolver las opciones de cada buffer — si no, el cambio solo
    // se vería en los archivos abiertos después de la recarga. Se resuelve de
    // nuevo entero (y no solo el ancho) para que la configuración por archivo
    // siga ganando sobre el valor del tema.
    for i in 0..app.buffers.len() {
        let opciones = resolve_options(
            &app.config,
            &app.theme,
            app.buffers[i].ed.filename.as_deref(),
        );
        apply_options(&mut app.buffers[i].ed, &opciones);
    }
    app.buffers[app.active].ed.status = match warnings.first() {
        Some(first) => format!("Tema recargado — {first}"),
        None => "Tema recargado".to_string(),
    };
}

/// Mientras la vista previa está activa el buffer es de solo lectura: las
/// teclas de navegación desplazan el documento renderizado y el resto no
/// hace nada, para no editar a ciegas algo que no se está viendo. Los
/// atajos con `Ctrl` (guardar, salir, la paleta, y el propio `^E`) siguen
/// funcionando: se dejan pasar al camino de siempre.
fn handle_preview_key(app: &mut App, key: KeyEvent, page_size: usize) {
    let ed = &mut app.buffers[app.active].ed;
    match key.code {
        KeyCode::Up => ed.preview_offset = ed.preview_offset.saturating_sub(1),
        KeyCode::Down => ed.preview_offset = ed.preview_offset.saturating_add(1),
        KeyCode::PageUp => ed.preview_offset = ed.preview_offset.saturating_sub(page_size),
        KeyCode::PageDown => ed.preview_offset = ed.preview_offset.saturating_add(page_size),
        KeyCode::Home => ed.preview_offset = 0,
        // `draw_preview` recorta al final real del documento renderizado,
        // que es quien conoce su largo.
        KeyCode::End => ed.preview_offset = usize::MAX / 2,
        KeyCode::Esc => {
            ed.preview = false;
            ed.status = "Vista previa desactivada".to_string();
        }
        _ => {
            ed.status = "Vista previa: solo lectura — ^E o Esc para volver a editar".to_string();
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent, page_size: usize) {
    let buf = &app.buffers[app.active];
    if buf.ed.preview
        && matches!(buf.ed.mode, Mode::Editing)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !matches!(key.code, KeyCode::F(_))
    {
        handle_preview_key(app, key, page_size);
        return;
    }
    match app.buffers[app.active].ed.mode {
        Mode::Editing => handle_layer_key(app, key, page_size),
        Mode::Prompt { .. } => handle_prompt_key(app, key),
        Mode::Completion { .. } => handle_completion_key(app, key),
        Mode::Palette { .. } => handle_palette_key(app, key),
        Mode::FilePicker { .. } => handle_file_picker_key(app, key),
        Mode::ProjectSearch { .. } => handle_project_search_key(app, key),
        Mode::Hover { .. } => handle_hover_key(app, key, page_size),
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

    // Las teclas de los plugins se miran antes que el perfil: un plugin que
    // ata Ctrl+J la pisa a propósito, y si se mirara después nunca ganaría
    // contra una tecla que Flint ya usa.
    let modal = matches!(app.buffers[app.active].ed.layer, Layer::ModalNormal);
    if let Some(&i) = app.plugin_keys.get(&(chord, modal)) {
        run_plugin_command(app, i);
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
                    dispatch_action(app, Action::InsertChar(c), page_size);
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
    // Las dos teclas de macro nunca entran en la grabación: la que la
    // termina quedaría adentro, y repetir la macro empezaría a grabar otra.
    if !matches!(action, Action::MacroRecord | Action::MacroPlay)
        && let Some(grabando) = app.macro_recording.as_mut()
    {
        grabando.push(action);
    }
    let was_select_line = matches!(action, Action::SelectLine);
    execute_action(app, action, page_size);
    app.last_was_select_line = was_select_line;
    // `(` y `,` abren la ayuda de firmas (o pasan al parámetro siguiente).
    // Con la ayuda abierta, cada tecla la vuelve a pedir: así sigue al
    // parámetro activo, y se cierra cuando el servidor contesta que el
    // cursor ya no está en ninguna llamada.
    let abre = matches!(action, Action::InsertChar('(' | ','));
    let edita = matches!(action, Action::InsertChar(_) | Action::Backspace | Action::DeleteForward);
    if abre || (edita && app.firma.is_some()) {
        pedir_firma(app, true);
    }
}

/// Empieza a grabar, o cierra la grabación en curso y la deja lista para
/// repetir. Una grabación vacía se descarta: dejarla pisaría la macro
/// anterior con nada, que nunca es lo que se quiso.
fn toggle_macro_recording(app: &mut App) {
    let status = match app.macro_recording.take() {
        Some(acciones) if acciones.is_empty() => {
            "Macro vacía, no se guardó nada".to_string()
        }
        Some(acciones) => {
            let n = acciones.len();
            app.macro_last = acciones;
            format!("Macro grabada ({n} acción(es)) — ^B la repite")
        }
        None => {
            app.macro_recording = Some(Vec::new());
            "Grabando macro — ^U termina".to_string()
        }
    };
    app.buffers[app.active].ed.status = status;
}

/// Repite la última macro. La grabación se aparta mientras corre: si no, una
/// macro reproducida durante una grabación entraría dos veces (una por la
/// acción de repetir y otra por cada acción repetida).
fn play_macro(app: &mut App, page_size: usize) {
    if app.macro_last.is_empty() {
        app.buffers[app.active].ed.status = "No hay ninguna macro grabada".to_string();
        return;
    }
    let acciones = std::mem::take(&mut app.macro_last);
    let grabando = app.macro_recording.take();
    for &accion in &acciones {
        execute_action(app, accion, page_size);
    }
    app.macro_recording = grabando;
    app.macro_last = acciones;
    let n = app.macro_last.len();
    app.buffers[app.active].ed.status = format!("Macro repetida ({n} acción(es))");
}

/// El único lugar que sabe qué hace cada `Action` — un perfil de teclado
/// nuevo, o un remapeo propio, solo necesitan decidir qué tecla dispara cuál;
/// el significado de la acción no cambia.
fn execute_action(app: &mut App, action: Action, page_size: usize) {
    match action {
        Action::Save => guardar(app, false),
        Action::Format => {
            if let Some(aviso) = formatear_ahora(app, false) {
                app.buffers[app.active].ed.status = aviso;
            }
        }
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
                    let destino = set_clipboard(app, text);
                    app.buffers[app.active].ed.status = format!("Copiado {destino}");
                }
                None => app.buffers[app.active].ed.status = "Nada seleccionado".to_string(),
            }
        }
        Action::SystemCut => match app.buffers[app.active].ed.selected_text() {
            Some(text) => {
                let destino = set_clipboard(app, text);
                app.buffers[app.active].ed.delete_selection_action();
                app.buffers[app.active].ed.status = format!("Cortado {destino}");
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
            registrar_salto(app);
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
        // Entre un par vacío, Backspace se lleva los dos: es el par que puso
        // el cierre automático, así que deshacerlo con una sola tecla es lo
        // que uno espera.
        Action::Backspace => {
            let ed = &mut app.buffers[app.active].ed;
            if !ed.backspace_pair() {
                ed.backspace();
            }
        }
        Action::DeleteForward => app.buffers[app.active].ed.delete_forward(),
        // Con una selección de varias líneas, Tab indenta el bloque en vez de
        // reemplazarlo por un tabulador — que es lo que haría cualquier otra
        // inserción, y nunca es lo que se quiso.
        Action::InsertTab => {
            let ed = &mut app.buffers[app.active].ed;
            if ed.selection_spans_lines() {
                ed.indent_lines();
            } else {
                ed.insert_indent();
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
        Action::EscapeKey => {
            // Con la ayuda de firmas abierta, Esc la cierra y nada más.
            if app.firma.take().is_none() {
                handle_escape(app);
            }
        }
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
        Action::TogglePreview => {
            let buf = &mut app.buffers[app.active];
            // La vista previa solo tiene sentido en Markdown: en cualquier
            // otro archivo el texto fuente ya es lo que hay que ver.
            if !matches!(buf.lang, Some(highlight::Lang::Markdown)) {
                buf.ed.status = "La vista previa es solo para archivos Markdown".to_string();
            } else {
                buf.ed.preview = !buf.ed.preview;
                buf.ed.preview_offset = 0;
                buf.ed.status = if buf.ed.preview {
                    "Vista previa — ↑↓/PgUp/PgDn desplazan, ^E o Esc vuelve a editar".to_string()
                } else {
                    "Vista previa desactivada".to_string()
                };
            }
        }
        Action::InsertChar(c) => app.buffers[app.active].ed.insert_char_pairing(c),
        Action::FindFilePrompt => open_file_picker(app),
        Action::SearchProjectPrompt => open_project_search(app),
        Action::JumpMatchingBracket => {
            let ed = &mut app.buffers[app.active].ed;
            ed.status = if ed.jump_to_matching_bracket() {
                "Al par".to_string()
            } else {
                "El cursor no está en un paréntesis, corchete o llave con pareja".to_string()
            };
        }
        Action::MacroRecord => toggle_macro_recording(app),
        Action::MacroPlay => play_macro(app, page_size),
        Action::GotoLinePrompt => {
            app.buffers[app.active].ed.mode = Mode::Prompt {
                kind: PromptKind::GotoLine,
                buffer: String::new(),
                label: "Ir a la línea: ".to_string(),
            };
        }
        Action::GotoDefinition => goto_definition(app),
        Action::Hover => pedir_hover(app),
        Action::SignatureHelp => pedir_firma(app, false),
        Action::JumpBack => saltar_en_la_lista(app, true),
        Action::JumpForward => saltar_en_la_lista(app, false),
        Action::RenamePrompt => rename_prompt(app),
        Action::ToggleComment => toggle_comment(app),
        Action::ToggleWrap => {
            let ed = &mut app.buffers[app.active].ed;
            ed.wrap = !ed.wrap;
            ed.col_offset = 0;
            // Sin ajuste no existe "media línea arriba del borde".
            ed.row_sub_offset = 0;
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
        // El didOpen ya lleva el texto de ahora: lo que se editó antes (por
        // ejemplo, mientras el servidor arrancaba) no se vuelve a mandar
        // como cambio.
        let buf = &mut app.buffers[idx];
        let _ = buf.ed.take_lsp_sync_plan();
        buf.lsp_synced_version = buf.ed.content_version;
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
            let opciones = resolve_options(&app.config, &app.theme, new_buf.ed.filename.as_deref());
            apply_options(&mut new_buf.ed, &opciones);
            if let Some(l) = &new_buf.lang {
                new_buf.ed.status = format!("{} — resaltado: {}", new_buf.ed.status, l.label());
            }
            if let Some(respaldo) = new_buf.ed.pending_backup() {
                new_buf.ed.status = format!(
                    "{} — ATENCIÓN: quedó un respaldo más nuevo en {}",
                    new_buf.ed.status,
                    respaldo.display()
                );
            }
            app.buffers.push(new_buf);
            app.active = app.buffers.len() - 1;
            try_attach_lsp(app, app.active);
            let count = app.buffers.len();
            app.buffers[app.active].ed.status =
                format!("{} — {count} buffer(s) abierto(s)", app.buffers[app.active].ed.status);
            // Al final, para que lo que diga el plugin sea lo que queda a la
            // vista.
            emitir_evento(app, plugins::Evento::Abrir);
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

/// Cuántos archivos entran en el índice. Un proyecto normal no llega ni
/// cerca; el tope está para que abrir el buscador en `/` o en un home entero
/// no se coma toda la memoria ni tarde un minuto.
const MAX_ARCHIVOS_INDEXADOS: usize = 20_000;
/// Hasta dónde bajar. Más profundo que esto, en un proyecto de verdad, ya es
/// caché de alguna herramienta.
const MAX_PROFUNDIDAD: usize = 12;

/// Directorios que nunca aportan un archivo que uno quiera editar a mano.
/// Recorrerlos es la diferencia entre un índice instantáneo y uno que tarda.
fn directorio_ignorado(nombre: &str) -> bool {
    matches!(
        nombre,
        ".git" | "target" | "node_modules" | ".venv" | "venv" | "__pycache__" | ".mypy_cache"
    )
}

/// Recorre el proyecto desde `raiz` y devuelve las rutas relativas de los
/// archivos, ordenadas. Sin dependencias: `read_dir` con una pila propia, que
/// además hace fácil respetar los topes de arriba.
fn indexar_archivos(raiz: &Path) -> Vec<String> {
    let mut pendientes = vec![(raiz.to_path_buf(), 0usize)];
    let mut encontrados: Vec<String> = Vec::new();
    while let Some((dir, profundidad)) = pendientes.pop() {
        if encontrados.len() >= MAX_ARCHIVOS_INDEXADOS {
            break;
        }
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entrada in entradas.flatten() {
            let ruta = entrada.path();
            let Some(nombre) = ruta.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Ok(tipo) = entrada.file_type() else { continue };
            if tipo.is_dir() {
                // Los enlaces simbólicos no se siguen: un enlace a un
                // directorio de más arriba haría un recorrido infinito.
                if !tipo.is_symlink()
                    && profundidad < MAX_PROFUNDIDAD
                    && !directorio_ignorado(nombre)
                    && !nombre.starts_with('.')
                {
                    pendientes.push((ruta, profundidad + 1));
                }
                continue;
            }
            if nombre.starts_with('.') {
                continue;
            }
            let relativa = ruta.strip_prefix(raiz).unwrap_or(&ruta);
            encontrados.push(relativa.to_string_lossy().to_string());
            if encontrados.len() >= MAX_ARCHIVOS_INDEXADOS {
                break;
            }
        }
    }
    encontrados.sort_unstable();
    encontrados
}

/// Lo que hace que un directorio sea "el proyecto" y no una carpeta más.
const MARCAS_DE_PROYECTO: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "Makefile",
];

/// La raíz del buscador. Se arranca en el directorio del archivo abierto (o
/// desde donde se lanzó Flint, si el buffer no tiene nombre) y se sube
/// mientras se encuentre una marca de proyecto, quedándose con la más alta:
/// abrir `src/main.rs` tiene que ofrecer todo el repositorio, no solo `src/`.
/// Sin ninguna marca, la raíz es el directorio del archivo — que es lo
/// conservador: en `/etc/hosts` uno no quiere indexar `/`.
fn raiz_del_proyecto(app: &App) -> PathBuf {
    let inicio = app.buffers[app.active]
        .ed
        .filename
        .as_ref()
        .and_then(|p| p.parent())
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let absoluto = std::fs::canonicalize(&inicio).unwrap_or(inicio.clone());

    let mut raiz = inicio;
    let mut actual = absoluto.as_path();
    loop {
        if MARCAS_DE_PROYECTO.iter().any(|m| actual.join(m).exists()) {
            raiz = actual.to_path_buf();
        }
        match actual.parent() {
            Some(padre) => actual = padre,
            None => break,
        }
    }
    raiz
}

fn open_file_picker(app: &mut App) {
    let raiz = raiz_del_proyecto(app);
    app.file_index = indexar_archivos(&raiz);
    if app.file_index.is_empty() {
        app.buffers[app.active].ed.status =
            format!("No encontré archivos en {}", raiz.display());
        return;
    }
    let n = app.file_index.len();
    app.buffers[app.active].ed.status = format!("{n} archivo(s) en {}", raiz.display());
    app.buffers[app.active].ed.mode = Mode::FilePicker {
        query: String::new(),
        selected: 0,
    };
}

fn filtered_file_indices(app: &App, query: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = app
        .file_index
        .iter()
        .enumerate()
        .filter_map(|(i, ruta)| fuzzy_score(query, ruta).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Mismo teclado que la paleta: escribir filtra, las flechas eligen, Enter
/// abre y Esc cancela.
fn handle_file_picker_key(app: &mut App, key: KeyEvent) {
    let (mut query, mut selected) =
        match std::mem::replace(&mut app.buffers[app.active].ed.mode, Mode::Editing) {
            Mode::FilePicker { query, selected } => (query, selected),
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
            let filtered = filtered_file_indices(app, &query);
            selected = selected
                .checked_sub(1)
                .unwrap_or(filtered.len().saturating_sub(1));
        }
        KeyCode::Down => {
            let filtered = filtered_file_indices(app, &query);
            if !filtered.is_empty() {
                selected = (selected + 1) % filtered.len();
            }
        }
        KeyCode::Enter => {
            let filtered = filtered_file_indices(app, &query);
            match filtered.get(selected).and_then(|&i| app.file_index.get(i)) {
                Some(relativa) => {
                    let ruta = raiz_del_proyecto(app).join(relativa);
                    open_file_into_new_buffer(app, ruta.to_string_lossy().to_string());
                    return;
                }
                None => {
                    // Sin coincidencias no se cierra: da lugar a corregir lo
                    // escrito, igual que la paleta.
                    app.buffers[app.active].ed.mode = Mode::FilePicker { query, selected };
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
    app.buffers[app.active].ed.mode = Mode::FilePicker { query, selected };
}

fn open_project_search(app: &mut App) {
    let raiz = raiz_del_proyecto(app);
    let rutas = indexar_archivos(&raiz);

    // Lo que tiene cada buffer abierto, por ruta relativa a la raíz: se
    // busca en eso y no en el disco, para que aparezca lo que uno escribió
    // aunque todavía no lo haya guardado.
    let raiz_abs = std::fs::canonicalize(&raiz).unwrap_or(raiz.clone());
    let abiertos: HashMap<String, String> = app
        .buffers
        .iter()
        .filter_map(|b| {
            let ruta = std::fs::canonicalize(b.ed.filename.as_ref()?).ok()?;
            let relativa = ruta.strip_prefix(&raiz_abs).ok()?;
            Some((relativa.to_string_lossy().to_string(), b.ed.rope.to_string()))
        })
        .collect();
    (app.search_files, app.search_partial) =
        busqueda::cargar(&raiz, &rutas, |r| abiertos.get(r).cloned());
    app.search_root = raiz;
    app.search_hits.clear();

    // Si hay una selección de una sola línea, se busca eso de entrada: es
    // el "¿dónde más se usa esto?" más rápido.
    let ed = &app.buffers[app.active].ed;
    let inicial = ed
        .selected_text()
        .filter(|t| !t.is_empty() && !t.contains('\n'))
        .unwrap_or_default();
    actualizar_busqueda(app, inicial, false);
}

/// Corre la consulta y deja el modo con el resultado. Se llama en cada
/// tecla: la búsqueda es sobre memoria, no sobre el disco.
fn actualizar_busqueda(app: &mut App, query: String, regex: bool) {
    let r = if !regex {
        Ok(busqueda::buscar(&app.search_files, &query))
    } else if query.is_empty() {
        Ok(busqueda::Resultado { coincidencias: Vec::new(), recortado: false, archivos: 0 })
    } else {
        busqueda::expresion(&query, true).map(|re| busqueda::buscar_con(&app.search_files, &re))
    };
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            // Mientras se escribe, la regex pasa por estados inválidos (un
            // paréntesis abierto): se dice por qué y se deja la lista vacía.
            app.search_hits = Vec::new();
            let resumen = format!("(regex inválida: {e})");
            app.buffers[app.active].ed.mode = Mode::ProjectSearch { query, selected: 0, resumen, regex };
            return;
        }
    };
    let mut resumen = if query.is_empty() {
        format!("({} archivo(s) en {})", app.search_files.len(), app.search_root.display())
    } else if r.coincidencias.is_empty() {
        "(sin coincidencias)".to_string()
    } else if r.recortado {
        format!("(las primeras {} — afiná la búsqueda)", r.coincidencias.len())
    } else {
        format!("({} en {} archivo(s))", r.coincidencias.len(), r.archivos)
    };
    if app.search_partial {
        resumen.push_str(" — proyecto muy grande: se leyó solo una parte");
    }
    app.search_hits = r.coincidencias;
    app.buffers[app.active].ed.mode = Mode::ProjectSearch { query, selected: 0, resumen, regex };
}

fn cerrar_busqueda(app: &mut App) {
    app.search_files = Vec::new();
    app.search_hits = Vec::new();
}

/// Escribir busca, las flechas (y RePág/AvPág) eligen, Enter salta a la
/// coincidencia y Esc cancela. Tab alterna entre texto y regex, y Ctrl+R
/// pasa a pedir con qué reemplazar.
fn handle_project_search_key(app: &mut App, key: KeyEvent) {
    let (mut query, mut selected, mut resumen, regex) =
        match std::mem::replace(&mut app.buffers[app.active].ed.mode, Mode::Editing) {
            Mode::ProjectSearch { query, selected, resumen, regex } => (query, selected, resumen, regex),
            other => {
                app.buffers[app.active].ed.mode = other;
                return;
            }
        };
    let total = app.search_hits.len();

    match key.code {
        KeyCode::Esc => {
            cerrar_busqueda(app);
            app.buffers[app.active].ed.status = "Cancelado".to_string();
            return;
        }
        KeyCode::Enter => {
            if let Some(c) = app.search_hits.get(selected).cloned() {
                saltar_a_coincidencia(app, &c, &query);
                cerrar_busqueda(app);
                return;
            }
        }
        KeyCode::Backspace => {
            query.pop();
            actualizar_busqueda(app, query, regex);
            return;
        }
        KeyCode::Tab => {
            actualizar_busqueda(app, query, !regex);
            return;
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            match preparar_reemplazo_en_proyecto(app, &query, regex) {
                Ok(()) => return,
                Err(por_que) => resumen = por_que,
            }
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            query.push(c);
            actualizar_busqueda(app, query, regex);
            return;
        }
        KeyCode::Up if total > 0 => selected = selected.checked_sub(1).unwrap_or(total - 1),
        KeyCode::Down if total > 0 => selected = (selected + 1) % total,
        KeyCode::PageUp => selected = selected.saturating_sub(10),
        KeyCode::PageDown if total > 0 => selected = (selected + 10).min(total - 1),
        _ => {}
    }
    app.buffers[app.active].ed.mode = Mode::ProjectSearch { query, selected, resumen, regex };
}

/// Cierra el buscador y pide con qué reemplazar. Si no hay nada que
/// reemplazar, el buscador sigue abierto y el error dice por qué.
fn preparar_reemplazo_en_proyecto(app: &mut App, query: &str, regex: bool) -> Result<(), String> {
    let re = busqueda::expresion(query, regex).map_err(|e| format!("(regex inválida: {e})"))?;
    let rutas: Vec<PathBuf> = if query.is_empty() {
        Vec::new()
    } else {
        busqueda::rutas_con(&app.search_files, &re)
            .into_iter()
            .map(|r| app.search_root.join(r))
            .collect()
    };
    if rutas.is_empty() {
        return Err("(nada que reemplazar)".to_string());
    }
    if rutas.len() > MAX_ARCHIVOS_RENOMBRE {
        return Err(format!(
            "(aparece en {} archivos, más del tope de {MAX_ARCHIVOS_RENOMBRE} para reemplazar; afiná la búsqueda)",
            rutas.len()
        ));
    }
    let parcial = if app.search_partial { " (del que se leyó)" } else { "" };
    let label = format!("Reemplazar \"{query}\" en {} archivo(s){parcial} por: ", rutas.len());
    cerrar_busqueda(app);
    app.buffers[app.active].ed.mode = Mode::Prompt {
        kind: PromptKind::ReplaceProject { consulta: query.to_string(), regex, rutas },
        buffer: String::new(),
        label,
    };
    Ok(())
}

/// Reemplaza en cada archivo todas las apariciones, no una por línea como
/// muestra la lista. Igual que el renombre, los archivos quedan abiertos y
/// sin guardar para revisarlos, y un Ctrl+Z deshace el reemplazo entero de
/// un archivo. Se calcula contra el texto de cada buffer, que es lo que se
/// buscó (lo abierto manda sobre el disco).
fn reemplazar_en_proyecto(app: &mut App, consulta: &str, regex: bool, rutas: &[PathBuf], con: &str) {
    let Ok(re) = busqueda::expresion(consulta, regex) else { return };
    let volver_a = app.active;
    let (mut archivos, mut total, mut abiertos) = (0usize, 0usize, 0usize);
    for ruta in rutas {
        let destino = std::fs::canonicalize(ruta).ok();
        let idx = match app.buffers.iter().position(|b| {
            destino.is_some() && b.ed.filename.as_ref().and_then(|f| std::fs::canonicalize(f).ok()) == destino
        }) {
            Some(i) => i,
            None => {
                let antes = app.buffers.len();
                open_file_into_new_buffer(app, ruta.display().to_string());
                if app.buffers.len() == antes {
                    continue;
                }
                abiertos += 1;
                app.buffers.len() - 1
            }
        };
        let ed = &mut app.buffers[idx].ed;
        let cambios = cambios_de_reemplazo(ed, &re, con, regex);
        if ed.replace_ranges(&cambios) > 0 {
            archivos += 1;
            total += cambios.len();
        }
    }
    app.active = volver_a.min(app.buffers.len().saturating_sub(1));
    let sin_guardar = if abiertos > 0 { format!(", {abiertos} abierto(s) sin guardar") } else { String::new() };
    app.buffers[app.active].ed.status = format!("Reemplazadas {total} aparición(es) en {archivos} archivo(s){sin_guardar}");
}

/// Los reemplazos de un buffer, pasados de bytes a posiciones del editor.
fn cambios_de_reemplazo(
    ed: &Editor,
    re: &regex::Regex,
    con: &str,
    regex: bool,
) -> Vec<(editor::Position, editor::Position, String)> {
    let texto = ed.rope.to_string();
    let posicion = |byte: usize| {
        let c = ed.rope.byte_to_char(byte);
        let linea = ed.rope.char_to_line(c);
        editor::Position { line: linea, col: c - ed.rope.line_to_char(linea) }
    };
    busqueda::reemplazos(&texto, re, con, regex)
        .into_iter()
        .map(|(a, b, nuevo)| (posicion(a), posicion(b), nuevo))
        .collect()
}

/// Lleva a la coincidencia: a la pestaña donde ya está abierto el archivo,
/// o a una nueva. Queda como última búsqueda, así Ctrl+F y Enter siguen a la
/// próxima aparición dentro del mismo archivo.
fn saltar_a_coincidencia(app: &mut App, c: &busqueda::Coincidencia, query: &str) {
    let Some(relativa) = app.search_files.get(c.archivo).map(|a| a.ruta.clone()) else { return };
    let ruta = app.search_root.join(&relativa);
    registrar_salto(app);
    let destino = std::fs::canonicalize(&ruta).ok();
    let abierto = app.buffers.iter().position(|b| {
        destino.is_some()
            && b.ed.filename.as_ref().and_then(|f| std::fs::canonicalize(f).ok()) == destino
    });
    match abierto {
        Some(i) => app.active = i,
        None => {
            let antes = app.buffers.len();
            open_file_into_new_buffer(app, ruta.to_string_lossy().to_string());
            if app.buffers.len() == antes {
                return; // no se pudo abrir; el estado ya dice por qué
            }
        }
    }
    let ed = &mut app.buffers[app.active].ed;
    ed.goto_line(c.linea);
    ed.cursor.col = c.col.min(ed.line_char_len(ed.cursor.line));
    ed.last_search = Some(query.to_string());
    ed.status = format!("{relativa}:{} — \"{query}\"", c.linea + 1);
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

/// Resuelve las teclas que pidieron los plugins. Un destino puede ser el id
/// de un comando de plugin o el nombre de una acción de Flint, y se busca en
/// ese orden: el plugin conoce sus propios ids, así que si eligió uno que
/// además es el nombre de una acción, gana el suyo (y se avisa, porque casi
/// seguro no era la intención).
fn resolver_binds(
    keymap: &mut keymap::Keymap,
    commands: &[plugins::PluginCommand],
    binds: &[plugins::PluginBind],
    errors: &mut Vec<String>,
) -> HashMap<(keymap::KeyChord, bool), usize> {
    let mut out = HashMap::new();
    for bind in binds {
        let chord = match keymap::parse_chord(&bind.tecla) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("flint.bind: {e}"));
                continue;
            }
        };
        if let Some(i) = commands.iter().position(|c| c.id == bind.destino) {
            if Action::from_name(&bind.destino).is_some() {
                errors.push(format!(
                    "flint.bind(\"{}\"): el comando \"{}\" se llama igual que una acción de Flint; se ató el del plugin",
                    bind.tecla, bind.destino
                ));
            }
            out.insert((chord, bind.modal), i);
            continue;
        }
        match Action::from_name(&bind.destino) {
            Some(action) => {
                if bind.modal {
                    keymap::rebind_normal(keymap, chord, Some(action));
                } else {
                    keymap::rebind_direct(keymap, chord, Some(action));
                }
            }
            None => errors.push(format!(
                "flint.bind(\"{}\"): no existe el comando ni la acción \"{}\"",
                bind.tecla, bind.destino
            )),
        }
    }
    out
}

fn run_plugin_command(app: &mut App, i: usize) {
    if app.plugins.is_none() || app.plugin_commands.get(i).is_none() {
        return;
    }
    let ctx = plugin_context(app);
    // El puente y el comando salen de `app` justo para la llamada: el script
    // no toca el editor, así que no hace falta que los dos préstamos vivan a
    // la vez que las mutaciones de después.
    let resultado = {
        let bridge = app.plugins.as_ref().expect("recién comprobado");
        let cmd = &app.plugin_commands[i];
        bridge.invoke(cmd, ctx)
    };
    match resultado {
        Ok(efectos) => {
            let mut dijo_algo = false;
            for efecto in efectos {
                if aplicar_efecto(app, efecto) {
                    dijo_algo = true;
                }
            }
            if !dijo_algo {
                app.buffers[app.active].ed.status = "Comando de plugin ejecutado".to_string();
            }
        }
        Err(e) => {
            let id = app.plugin_commands[i].id.clone();
            app.buffers[app.active].ed.status = format!("Error en el plugin \"{id}\": {e}");
        }
    }
}

/// Corre los manejadores de `evento` sobre el buffer activo y aplica lo que
/// pidan. Si alguno falló, lo dice la barra de estado.
fn emitir_evento(app: &mut App, evento: plugins::Evento) {
    let escucha = app.plugins.as_ref().is_some_and(|b| b.escucha(evento));
    if app.en_evento || !escucha {
        return;
    }
    app.en_evento = true;
    let ctx = plugin_context(app);
    let (efectos, errores) = app.plugins.as_ref().expect("recién comprobado").emitir(evento, ctx);
    for efecto in efectos {
        aplicar_efecto(app, efecto);
    }
    if let Some(e) = errores.first() {
        app.buffers[app.active].ed.status = format!("Error en un plugin (evento {}): {e}", evento.nombre());
    }
    app.en_evento = false;
}

/// Avisa `save` por cada buffer que se guardó desde la última vuelta. Lo que
/// guarden los propios manejadores no vuelve a avisar.
fn avisar_guardados(app: &mut App) {
    let activo = app.active;
    for i in 0..app.buffers.len() {
        if !std::mem::take(&mut app.buffers[i].ed.recien_guardado) {
            continue;
        }
        app.active = i;
        emitir_evento(app, plugins::Evento::Guardar);
        if let Some(b) = app.buffers.get_mut(i) {
            b.ed.recien_guardado = false;
        }
    }
    app.active = activo.min(app.buffers.len().saturating_sub(1));
}

/// La foto del editor que ve un comando de plugin.
fn plugin_context(app: &mut App) -> plugins::PluginContext {
    let clipboard = get_clipboard_text(app).unwrap_or_default();
    let ed = &app.buffers[app.active].ed;
    plugins::PluginContext {
        filename: ed
            .filename
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        language: app.buffers[app.active]
            .lang
            .map(|l| l.id().to_string())
            .unwrap_or_default(),
        line_count: ed.line_count(),
        // De 0 a 1: adentro Flint cuenta desde 0, pero la API de plugins
        // cuenta desde 1, que es lo que muestra la barra de estado y lo que
        // espera cualquiera que escriba Lua.
        cursor_line: ed.cursor.line + 1,
        cursor_col: ed.cursor.col + 1,
        selection: ed.selected_text().unwrap_or_default(),
        text: ed.rope.to_string(),
        clipboard,
    }
}

/// Aplica un pedido de un plugin. Devuelve si dejó un texto en la barra de
/// estado, para no pisarlo después con el aviso genérico.
fn aplicar_efecto(app: &mut App, efecto: plugins::PluginEffect) -> bool {
    use plugins::PluginEffect as E;
    match efecto {
        E::Status(texto) => {
            app.buffers[app.active].ed.status = texto;
            true
        }
        E::InsertText(texto) => {
            for c in texto.chars() {
                app.buffers[app.active].ed.insert_char(c);
            }
            false
        }
        E::ReplaceSelection(texto) => {
            app.buffers[app.active].ed.delete_selection_action();
            for c in texto.chars() {
                app.buffers[app.active].ed.insert_char(c);
            }
            false
        }
        E::SetClipboard(texto) => {
            set_clipboard(app, texto);
            false
        }
        E::SetCursor { line, col } => {
            // De 1 a 0, la inversa de `plugin_context`.
            app.buffers[app.active].ed.goto_line(line.saturating_sub(1));
            let max = app.buffers[app.active]
                .ed
                .line_char_len(app.buffers[app.active].ed.cursor.line);
            app.buffers[app.active].ed.cursor.col = col.saturating_sub(1).min(max);
            false
        }
        E::Action(nombre) => match Action::from_name(&nombre) {
            Some(action) => {
                let page_size = app.text_area.height.max(1) as usize;
                dispatch_action(app, action, page_size);
                false
            }
            None => {
                app.buffers[app.active].ed.status =
                    format!("El plugin pidió la acción \"{nombre}\", que no existe");
                true
            }
        },
    }
}

fn normal_delete(app: &mut App) {
    // "Borrar" corta: lo que se va a borrar (el rango si hay selección, si no
    // el carácter siguiente en cada cursor) queda guardado en el registro y
    // en el portapapeles del sistema antes de borrarlo — igual que la `d` de
    // Vim/Kakoune, recuperable con `p` después. Actúa sobre todos los
    // cursores a la vez, nunca es un no-op.
    let destino = match app.buffers[app.active].ed.text_at_forward_delete_points() {
        Some(text) => set_clipboard(app, text),
        None => "al registro interno".to_string(),
    };
    app.buffers[app.active].ed.delete_at_each_selection();
    app.buffers[app.active].ed.status = format!("Cortado {destino}");
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
            let destino = set_clipboard(app, text);
            app.buffers[app.active].ed.status = format!("Copiado {destino}");
        }
        None => app.buffers[app.active].ed.status = "Nada seleccionado".to_string(),
    }
}

/// Comentar es lo único de la edición que necesita saber en qué lenguaje
/// está el archivo, así que vive acá (donde el buffer conoce su `Lang`) y no
/// en el editor, que trabaja con texto a secas.
fn toggle_comment(app: &mut App) {
    let buf = &mut app.buffers[app.active];
    let Some(token) = buf.lang.and_then(|l| l.line_comment()) else {
        buf.ed.status = match buf.lang {
            Some(l) => format!("{} no tiene comentarios de una línea", l.label()),
            None => "No sé en qué lenguaje está este archivo".to_string(),
        };
        return;
    };
    buf.ed.status = if buf.ed.toggle_line_comment(token) {
        format!("Comentario alternado con \"{token}\"")
    } else {
        "No hay líneas con texto para comentar".to_string()
    };
}

fn normal_paste(app: &mut App) {
    paste_from_clipboard(app);
}

/// Guarda `text` en el registro interno y, según el modo configurado, en el
/// portapapeles del sistema y/o en el de la terminal (OSC 52).
///
/// Devuelve a dónde llegó, en palabras, en vez de escribir el estado: el que
/// llama tiene que poder decir "Copiado …" o "Cortado …" con el mismo dato.
/// Antes esta función escribía el estado y el que llamaba lo pisaba una línea
/// después, así que un fallo del portapapeles no se veía nunca.
fn set_clipboard(app: &mut App, text: String) -> String {
    use clipboard::Mode;
    let modo = app.clipboard_mode;
    let mut destinos: Vec<&str> = Vec::new();
    let mut fallo: Option<String> = None;

    if matches!(modo, Mode::Auto | Mode::System) {
        match app.clipboard.as_mut() {
            Some(cb) => match cb.set_text(text.clone()) {
                Ok(()) => destinos.push("al portapapeles del sistema"),
                Err(e) => fallo = Some(e.to_string()),
            },
            None => fallo = Some("no hay portapapeles del sistema en esta sesión".to_string()),
        }
    }

    // En `auto`, la terminal es el plan B: si el del sistema anduvo no hace
    // falta molestar a la terminal, y si no anduvo (una sesión SSH sin
    // display es el caso típico) es el único camino que queda.
    let por_terminal = modo == Mode::Terminal || (modo == Mode::Auto && destinos.is_empty());
    if por_terminal {
        match clipboard::copy_via_terminal(&text) {
            Ok(()) => destinos.push("al portapapeles de la terminal"),
            Err(e) => fallo = Some(e),
        }
    }

    app.buffers[app.active].ed.register = Some(text);

    match (destinos.is_empty(), fallo) {
        (false, _) => destinos.join(" y "),
        (true, Some(e)) => format!("solo al registro interno ({e})"),
        (true, None) => "al registro interno".to_string(),
    }
}

/// El texto a pegar: el portapapeles del sistema si el modo lo permite y
/// tiene algo (así se puede pegar lo copiado en cualquier otra app), si no
/// el registro interno de Flint.
///
/// Con el modo `terminal` no se lee del sistema a propósito: si se eligió la
/// terminal es porque el portapapeles del sistema no es el que importa, y
/// leer de él pegaría algo distinto de lo último que se copió. OSC 52 tiene
/// una consulta para leer, pero casi ninguna terminal la habilita y la
/// respuesta llegaría mezclada con las teclas.
fn get_clipboard_text(app: &mut App) -> Option<String> {
    use clipboard::Mode;
    if matches!(app.clipboard_mode, Mode::Auto | Mode::System)
        && let Some(cb) = app.clipboard.as_mut()
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
    let idx = app.active;
    let buf = &app.buffers[idx];
    let Some(highlighter) = buf.highlighter.as_ref() else {
        app.buffers[idx].ed.status = "Sin árbol de sintaxis para este archivo".to_string();
        return;
    };
    // El árbol que el resaltado ya tiene en caché sirve tal cual mientras el
    // buffer no haya cambiado desde esa pasada, que acá es lo normal: en
    // NORMAL no se está escribiendo. Solo si no corresponde se analiza de
    // nuevo el archivo entero, que es lo que antes pasaba siempre.
    let propio;
    let tree = match highlighter.arbol_de(&buf.ed.rope) {
        Some(t) => t,
        None => {
            let text = buf.ed.rope.to_string();
            match highlighter.parse(&text) {
                Some(t) => {
                    propio = t;
                    &propio
                }
                None => {
                    app.buffers[idx].ed.status = "No se pudo analizar el archivo".to_string();
                    return;
                }
            }
        }
    };

    let mut sels = buf.ed.selections_snapshot();
    let len_chars = buf.ed.rope.len_chars();
    let mut changed = 0;
    for sel in sels.iter_mut() {
        let (start_char, end_char) = buf.ed.selection_char_range(*sel);
        let end_char = end_char.max(start_char + 1).min(len_chars);
        let start_byte = buf.ed.rope.char_to_byte(start_char);
        let end_byte = buf.ed.rope.char_to_byte(end_char);
        if let Some((nb_start, nb_end)) = highlight::expand_selection(tree, start_byte, end_byte) {
            let new_start = buf.ed.position_from_char_idx(buf.ed.rope.byte_to_char(nb_start));
            let new_end = buf.ed.position_from_char_idx(buf.ed.rope.byte_to_char(nb_end));
            *sel = editor::Selection {
                anchor: new_start,
                cursor: new_end,
            };
            changed += 1;
        }
    }
    let ed = &mut app.buffers[idx].ed;
    ed.apply_selections(sels);
    ed.status = if changed > 0 {
        "Selección expandida al nodo padre".to_string()
    } else {
        "No se pudo expandir la selección".to_string()
    };
}

/// Autocompletado sin servidor de lenguaje: las palabras que ya están
/// escritas en el propio buffer (como `Ctrl+N` en Vim). Es lo único que se
/// puede saber de un `.txt`, y en prosa es justo lo útil — nombres propios,
/// términos técnicos y palabras largas que ya usaste más arriba.
fn complete_from_buffer(app: &mut App) {
    complete_from_buffer_in(&mut app.buffers[app.active].ed);
}

/// El trabajo real, sobre un `Editor` puntual — así también lo puede usar la
/// respuesta del LSP cuando el servidor no devolvió ninguna sugerencia, que
/// es cuando más se agradece tener algo en vez de nada.
fn complete_from_buffer_in(ed: &mut Editor) {
    let (trigger, prefix) = ed.identifier_prefix_before_cursor();
    let words = ed.words_starting_with(&prefix, 50);
    if words.is_empty() {
        ed.status = if prefix.is_empty() {
            "No hay palabras en el texto para sugerir".to_string()
        } else {
            format!("Ninguna palabra del texto empieza con \"{prefix}\"")
        };
        return;
    }
    ed.status = format!("{} sugerencia(s) del texto", words.len());
    ed.mode = Mode::Completion {
        items: words
            .into_iter()
            .map(|label| editor::CompletionEntry { label, ..Default::default() })
            .collect(),
        selected: 0,
        trigger,
        prefix,
    };
}

fn trigger_completion(app: &mut App) {
    // Sin servidor no hay que quedarse sin autocompletado: se cae a las
    // palabras del propio texto, que funcionan en cualquier archivo.
    if app.buffers[app.active].lsp_lang_id.is_none() {
        complete_from_buffer(app);
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
        complete_from_buffer(app);
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
/// Una entrada de la lista de autocompletado, tal como la manda el servidor.
///
/// Los tres campos de los que puede salir el texto a insertar están en el
/// spec y cada servidor prefiere uno: `rust-analyzer` suele mandar
/// `textEdit`, `pylsp` manda `insertText`, y varios mandan solo `label`. Se
/// prueban en ese orden — `textEdit` es el más específico y el único que el
/// spec dice que gana sobre los otros.
fn completion_entry_from(it: &Value) -> Option<editor::CompletionEntry> {
    let label = it.get("label").and_then(Value::as_str)?.to_string();
    let detail = it.get("detail").and_then(Value::as_str).map(str::to_string);
    // `insertTextFormat` 2 = Snippet (`$0`, `${1:nombre}`…) — Flint no
    // expande snippets, así que ahí se ignora el texto propuesto y se
    // inserta la etiqueta, que es texto plano.
    if it.get("insertTextFormat").and_then(Value::as_u64) == Some(2) {
        return Some(editor::CompletionEntry {
            label,
            detail,
            ..Default::default()
        });
    }
    // `insert` es la forma de InsertReplaceEdit; Flint no la anuncia, pero
    // si un servidor la manda igual, se usa el rango de insertar, que es el
    // que no pisa lo que hay después del cursor.
    let lsp_range = it.get("textEdit").and_then(|e| e.get("range").or_else(|| e.get("insert"))).and_then(|r| {
        Some((pos_lsp(r.get("start")?)?, pos_lsp(r.get("end")?)?))
    });
    let insert_text = it
        .get("textEdit")
        .and_then(|e| e.get("newText"))
        .and_then(Value::as_str)
        .or_else(|| it.get("insertText").and_then(Value::as_str))
        .map(str::to_string);
    Some(editor::CompletionEntry {
        label,
        detail,
        insert_text,
        lsp_range,
        reemplazo: None,
    })
}

/// Pasa el rango de `textEdit` de una entrada a columnas en caracteres
/// (`CompletionEntry::reemplazo`). `trigger` y `prefix` son los del pedido:
/// el cursor estaba en `trigger.col + largo de prefix`.
///
/// El texto pudo cambiar entre el pedido y la respuesta, pero solo con lo
/// que se escribió en el cursor: lo anterior al cursor del pedido sigue
/// igual (así se convierte el inicio), y lo que el rango abarcaba después
/// de ese cursor ahora está después del cursor actual (así se cuenta el
/// fin). Un rango en otra línea no respeta el spec y se ignora.
fn reemplazo_de(ed: &Editor, trigger: (usize, usize), prefix: &str, rango: ((usize, usize), (usize, usize))) -> Option<(usize, usize)> {
    let ((l0, c0), (l1, c1)) = rango;
    if l0 != trigger.0 || l1 != trigger.0 {
        return None;
    }
    let col_pedido = trigger.1 + prefix.chars().count();
    let desde = ed.utf16_col_to_char(l0, c0).min(col_pedido);
    let pedido16 = ed.char_col_to_utf16(l0, col_pedido);
    let mut resto16 = c1.saturating_sub(pedido16);
    let col_cursor = if ed.cursor.line == l0 { ed.cursor.col } else { col_pedido };
    let mut despues = 0;
    for c in ed.rope.line(l0).chars().skip(col_cursor) {
        if resto16 == 0 || c == '\n' {
            break;
        }
        resto16 = resto16.saturating_sub(c.len_utf16());
        despues += 1;
    }
    Some((desde, despues))
}

fn filtered_completion_indices(items: &[editor::CompletionEntry], prefix: &str) -> Vec<usize> {
    let mut scored: Vec<(i32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| fuzzy_score(prefix, &it.label).map(|s| (s, i)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Reemplaza lo tipeado por el texto de `entry`: desde `trigger` (o desde
/// donde empiece el rango del servidor, si mandó uno) hasta el cursor, más
/// lo que ese rango abarque después del cursor.
fn aceptar_sugerencia(ed: &mut Editor, entry: &editor::CompletionEntry, trigger: editor::Position) {
    let text = entry.insert_text.clone().unwrap_or_else(|| entry.label.clone());
    let (desde, despues) = entry.reemplazo.unwrap_or((trigger.col, 0));
    ed.selection_anchor = Some(editor::Position { line: trigger.line, col: desde });
    ed.cursor = ed.clamp_position(editor::Position {
        line: ed.cursor.line,
        col: ed.cursor.col + despues,
    });
    ed.secondary.clear();
    ed.delete_selection_action();
    for c in text.chars() {
        ed.insert_char(c);
    }
    ed.status = format!("Insertado: {}", entry.label);
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
            aceptar_sugerencia(&mut app.buffers[app.active].ed, &items[idx], trigger);
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
                    's' | 'S' => guardar(app, true),
                    'n' | 'N' => app.buffers[app.active].ed.should_quit = true,
                    _ => app.buffers[app.active].ed.mode = Mode::Prompt { kind, buffer, label },
                }
            } else if let PromptKind::OverwriteConfirm { then_quit } = &kind {
                let then_quit = *then_quit;
                match c {
                    's' | 'S' => save_forcing(&mut app.buffers[app.active].ed, then_quit),
                    'n' | 'N' => {
                        app.buffers[app.active].ed.status =
                            "No se guardó; lo del disco quedó como estaba".to_string();
                    }
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
        PromptKind::Rename { palabra } => pedir_renombre(app, palabra, buffer),
        PromptKind::ReplaceProject { consulta, regex, rutas } => {
            reemplazar_en_proyecto(app, &consulta, regex, &rutas, &buffer)
        }
        PromptKind::GotoLine => {
            registrar_salto(app);
            submit_prompt_editor(&mut app.buffers[app.active].ed, PromptKind::GotoLine, buffer);
        }
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
    app.buffers[idx].ed.indent_after_colon = new_lang.is_some_and(|l| l.indents_after_colon());
    app.buffers[idx].ed.highlights_dirty = true;
    if let Some(l) = &app.buffers[idx].lang {
        app.buffers[idx].ed.status = format!("{} — resaltado: {}", app.buffers[idx].ed.status, l.label());
    }
    try_attach_lsp(app, idx);
}

fn submit_prompt_editor(ed: &mut Editor, kind: PromptKind, buffer: String) {
    match kind {
        // `Rename` lo atiende `submit_prompt`, que tiene acceso a `App`: el
        // servidor puede devolver cambios en varios archivos, no solo en
        // este buffer. Acá no llega nunca.
        PromptKind::Rename { .. } | PromptKind::ReplaceProject { .. } => {}
        PromptKind::GotoLine => match buffer.trim().parse::<usize>() {
            // Se cuenta desde 1 porque es como se cuenta en la barra de
            // estado y en cualquier mensaje de error de un compilador.
            Ok(n) if n >= 1 => {
                ed.goto_line(n - 1);
                let real = ed.cursor.line + 1;
                ed.status = if real == n {
                    format!("Línea {n}")
                } else {
                    format!("El archivo termina en la línea {real}")
                };
            }
            _ => ed.status = format!("\"{}\" no es un número de línea", buffer.trim()),
        },
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
        PromptKind::QuitConfirm | PromptKind::OverwriteConfirm { .. } => {
            ed.status = "Cancelado".to_string();
        }
        PromptKind::OpenFile | PromptKind::SaveAs { .. } => {
            unreachable!("interceptados antes, en submit_prompt")
        }
    }
}

/// Guarda sin volver a preguntar por el disco — es la rama del "sí" de
/// `OverwriteConfirm`, y también el camino normal cuando no hubo cambios
/// afuera.
fn save_forcing(ed: &mut Editor, then_quit: bool) {
    match ed.save() {
        Ok(()) => {
            ed.status = "Guardado".to_string();
            if then_quit {
                ed.should_quit = true;
            }
        }
        Err(e) => ed.status = format!("Error al guardar: {e}"),
    }
}

/// Cuánto se espera al servidor para formatear. Guardar no puede quedar
/// colgado de un servidor lento: pasado esto se guarda sin formatear.
const ESPERA_FORMATO: Duration = Duration::from_secs(2);

/// Guardar el buffer activo, formateándolo antes si `format_on_save` está
/// prendido para ese archivo. Si el formato no se pudo, se guarda igual y el
/// motivo queda en la barra de estado junto al "Guardado".
fn guardar(app: &mut App, then_quit: bool) {
    emitir_evento(app, plugins::Evento::AntesDeGuardar);
    let aviso = if app.buffers[app.active].ed.format_on_save {
        formatear_ahora(app, true)
    } else {
        None
    };
    let ed = &mut app.buffers[app.active].ed;
    try_save(ed, then_quit);
    if let Some(aviso) = aviso
        && matches!(ed.mode, Mode::Editing)
    {
        ed.status = format!("{} — {aviso}", ed.status);
    }
}

/// Le pide al servidor que formatee el buffer activo y aplica lo que
/// conteste, como un solo paso de deshacer. Espera la respuesta en el
/// momento, hasta `ESPERA_FORMATO`: al guardar, lo que se escribe tiene que
/// ser el texto ya formateado, y un pedido que vuelve cuando el archivo ya
/// se guardó llega tarde. Mientras espera, lo demás que mande el servidor
/// (diagnósticos, sobre todo) se atiende normalmente.
///
/// Devuelve qué decir en la barra de estado, o `None` si no hay nada que
/// contar. `al_guardar` calla los casos en que no hay con qué formatear (un
/// archivo sin servidor): ahí la opción simplemente no aplica.
fn formatear_ahora(app: &mut App, al_guardar: bool) -> Option<String> {
    let sin_servidor = app.lsp.is_none() || app.buffers[app.active].doc_uri.is_none();
    if sin_servidor || !app.lsp_format {
        if al_guardar {
            return None;
        }
        return Some(if sin_servidor {
            "Formatear necesita un servidor de lenguaje, y este archivo no tiene".to_string()
        } else {
            "El servidor de lenguaje no sabe formatear".to_string()
        });
    }
    let uri = app.buffers[app.active].doc_uri.clone()?;
    sync_lsp_ahora(app);
    let (tab, espacios) = {
        let ed = &app.buffers[app.active].ed;
        (ed.tab_width, ed.indent_with_spaces)
    };
    // Un servidor puede contestar "el contenido cambió" (ContentModified) o
    // "cancelado" si recibió el texto nuevo justo antes del pedido y todavía
    // no terminó de procesarlo: rust-analyzer lo hace cuando se guarda dos
    // veces seguidas con formato. El protocolo dice que el cliente reintente,
    // así que se reintenta, dentro de la misma espera total.
    const CONTENIDO_MODIFICADO: i64 = -32801;
    const PEDIDO_CANCELADO: i64 = -32800;
    let limite = Instant::now() + ESPERA_FORMATO;
    let a_tiempo = || {
        Some(format!(
            "el servidor no formateó en {} s; quedó sin formatear",
            ESPERA_FORMATO.as_secs()
        ))
    };
    let respuesta = loop {
        let enviado = app.lsp.as_mut().map(|c| c.request_formatting(&uri, tab, espacios));
        let id = match enviado {
            Some(Ok(id)) => id,
            Some(Err(e)) => return Some(format!("no pude pedirle el formato al servidor: {e}")),
            None => return None,
        };
        let Some(respuesta) = esperar_respuesta(app, id, limite.saturating_duration_since(Instant::now())) else {
            return a_tiempo();
        };
        let codigo = respuesta.get("error").and_then(|e| e.get("code")).and_then(Value::as_i64);
        if matches!(codigo, Some(CONTENIDO_MODIFICADO | PEDIDO_CANCELADO)) {
            if Instant::now() + Duration::from_millis(50) >= limite {
                return a_tiempo();
            }
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        break respuesta;
    };
    if let Some(error) = respuesta.get("error") {
        let mensaje = error.get("message").and_then(Value::as_str).unwrap_or("error desconocido");
        return Some(format!("el servidor no pudo formatear: {mensaje}"));
    }
    // `null` no dice por qué: rust-analyzer lo contesta tanto cuando el
    // archivo ya está formateado como cuando no pudo correr rustfmt (sin
    // instalar, o un error de sintaxis que rustfmt no sabe leer). No hay
    // forma de distinguirlos desde acá, así que el mensaje no afirma ninguna
    // de las dos.
    let Some(ediciones) = respuesta.get("result").and_then(Value::as_array).cloned() else {
        return Some("el servidor no propuso cambios".to_string());
    };
    let ed = &mut app.buffers[app.active].ed;
    let antes = ed.rope.clone();
    if aplicar_ediciones(ed, &ediciones) && ed.rope != antes {
        Some("formateado".to_string())
    } else {
        Some("ya estaba formateado".to_string())
    }
}

/// Espera la respuesta a la petición `id`, hasta `timeout`, y la devuelve
/// entera (con `result` o `error`). A diferencia de `wait_for_response`, que
/// solo se usa al arrancar, los demás mensajes que lleguen mientras tanto no
/// se tiran: pasan por `handle_lsp_message` como en el bucle principal.
fn esperar_respuesta(app: &mut App, id: u64, timeout: Duration) -> Option<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let ahora = Instant::now();
        if ahora >= deadline {
            return None;
        }
        let client = app.lsp.as_mut()?;
        if !client.is_alive() {
            return None;
        }
        let Some(msg) = client.recv_timeout((deadline - ahora).min(Duration::from_millis(50))) else {
            continue;
        };
        let es_la_respuesta = msg.get("method").is_none() && msg.get("id").and_then(Value::as_u64) == Some(id);
        if es_la_respuesta {
            client.pending.remove(&id);
            return Some(msg);
        }
        handle_lsp_message(app, msg);
    }
}

fn try_save(ed: &mut Editor, then_quit: bool) {
    if ed.filename.is_some() {
        // Otro proceso pudo haber tocado el archivo mientras estaba abierto
        // (un `git checkout`, un formateador). Guardar encima sin avisar
        // borraría eso, así que se pregunta una vez.
        if ed.disk_changed() {
            ed.mode = Mode::Prompt {
                kind: PromptKind::OverwriteConfirm { then_quit },
                buffer: String::new(),
                label: "El archivo cambió en el disco. ¿Guardar igual y pisarlo? (s = sí, n = no): "
                    .to_string(),
            };
            return;
        }
        save_forcing(ed, then_quit);
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
    // Un clic cierra el hover, igual que una tecla: la ventana habla del
    // lugar donde estaba el cursor, y el clic lo va a mover. La rueda no,
    // para poder leerlo sin que se vaya.
    if matches!(app.buffers[app.active].ed.mode, Mode::Hover { .. })
        && matches!(m.kind, MouseEventKind::Down(_))
    {
        app.buffers[app.active].ed.mode = Mode::Editing;
        app.hover_lines.clear();
    }
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

/// Un `App` para los tests: el buffer dado, sin servidor de lenguaje, sin
/// plugins y con la configuración de fábrica. Cada test le cambia lo que
/// necesita.
#[cfg(test)]
fn app_de_prueba(buffer: Buffer) -> App {
    App {
        buffers: vec![buffer],
        active: 0,
        text_area: Rect { x: 0, y: 1, width: 80, height: 20 },
        tabs_area: None,
        lsp: None,
        lsp_arrancando: None,
        lsp_lang: None,
        lsp_incremental: false,
        lsp_definition: false,
        lsp_rename: false,
        lsp_hover: false,
        lsp_format: false,
        lsp_firma: false,
        firma: None,
        lsp_forzar_sync: false,
        last_was_select_line: false,
        keymap: keymap::flint_profile(),
        pending_prefix: None,
        palette_entries: builtin_palette_entries(),
        plugin_commands: Vec::new(),
        plugins: None,
        plugin_keys: HashMap::new(),
        theme: theme::Theme::default(),
        theme_path: None,
        theme_mtime: None,
        theme_next_check: Instant::now(),
        clipboard_mode: clipboard::Mode::Internal,
        macro_recording: None,
        macro_last: Vec::new(),
        file_index: Vec::new(),
        search_files: Vec::new(),
        search_root: PathBuf::new(),
        search_partial: false,
        search_hits: Vec::new(),
        saltos: saltos::ListaSaltos::default(),
        hover_lines: Vec::new(),
        config: config::Config::default(),
        clipboard: None,
        preview_lines: Vec::new(),
        preview_key: None,
        en_evento: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Las tres formas en que un servidor dice qué insertar. Las dos
    /// primeras son respuestas reales, recortadas: `pylsp` (jedi) y
    /// `rust-analyzer`.
    #[test]
    fn el_texto_a_insertar_sale_del_campo_que_mande_cada_servidor() {
        let pylsp = json!({
            "label": "pardir",
            "kind": 5,
            "detail": "os",
            "insertText": "pardir",
            "sortText": "apardir"
        });
        let e = completion_entry_from(&pylsp).unwrap();
        assert_eq!(e.insert_text.as_deref(), Some("pardir"));
        assert_eq!(e.detail.as_deref(), Some("os"));

        // textEdit gana sobre insertText: es lo que dice el spec, y es lo
        // que manda rust-analyzer cuando los dos vienen distintos.
        let con_edit = json!({
            "label": "push",
            "insertText": "push",
            "textEdit": {
                "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 6}},
                "newText": "push()"
            }
        });
        assert_eq!(
            completion_entry_from(&con_edit).unwrap().insert_text.as_deref(),
            Some("push()")
        );

        // Solo label: se inserta la etiqueta.
        let pelado = json!({ "label": "len" });
        assert_eq!(completion_entry_from(&pelado).unwrap().insert_text, None);

        // Un snippet no se expande: se ignora su texto y queda la etiqueta.
        let snippet = json!({
            "label": "println!",
            "insertTextFormat": 2,
            "textEdit": { "newText": "println!(\"$1\")" }
        });
        assert_eq!(completion_entry_from(&snippet).unwrap().insert_text, None);

        // Sin label no hay entrada posible.
        assert!(completion_entry_from(&json!({ "detail": "x" })).is_none());
    }

    /// Acepta la primera entrada de `respuesta` en `texto`, con el cursor en
    /// `col` de la primera línea, como si se hubiera pedido ahí.
    fn aceptar_en(texto: &str, col: usize, respuesta: Value) -> String {
        let mut ed = Editor::open(None).expect("editor vacío");
        ed.rope = ropey::Rope::from_str(texto);
        ed.cursor = editor::Position { line: 0, col };
        let (trigger, prefix) = ed.identifier_prefix_before_cursor();
        let mut entry = completion_entry_from(&respuesta).unwrap();
        entry.reemplazo = entry
            .lsp_range
            .and_then(|r| reemplazo_de(&ed, (trigger.line, trigger.col), &prefix, r));
        aceptar_sugerencia(&mut ed, &entry, trigger);
        ed.rope.to_string()
    }

    fn con_rango(inicio: usize, fin: usize, nuevo: &str) -> Value {
        json!({
            "label": nuevo,
            "textEdit": {
                "range": {"start": {"line": 0, "character": inicio}, "end": {"line": 0, "character": fin}},
                "newText": nuevo
            }
        })
    }

    #[test]
    fn el_rango_de_textedit_decide_que_se_reemplaza() {
        // Rango que empieza antes de la palabra: el servidor reemplaza
        // `std::` entero, no solo `fo`.
        assert_eq!(aceptar_en("use std::fo;", 11, con_rango(4, 11, "std::fmt")), "use std::fmt;");
        // Rango que sigue después del cursor: se come el resto de la palabra.
        assert_eq!(aceptar_en("x.pu_old()", 4, con_rango(2, 8, "push")), "x.push()");
        // Rango que coincide con la palabra: igual que sin rango.
        assert_eq!(aceptar_en("let v = ve", 10, con_rango(8, 10, "vec!")), "let v = vec!");
        // Sin rango, lo de siempre: desde el comienzo de la palabra.
        assert_eq!(aceptar_en("let v = ve", 10, json!({"label": "vec"})), "let v = vec");
        // Un rango en otra línea no respeta el spec: se ignora.
        let otra_linea = json!({
            "label": "vec",
            "textEdit": {"range": {"start": {"line": 3, "character": 0}, "end": {"line": 3, "character": 2}}, "newText": "vec"}
        });
        assert_eq!(aceptar_en("let v = ve", 10, otra_linea), "let v = vec");
    }

    #[test]
    fn el_rango_de_textedit_se_mide_en_utf16() {
        // El emoji es un carácter pero dos unidades UTF-16: el 3..7 del
        // servidor es "abcd", no " abc" como sería contando caracteres.
        assert_eq!(aceptar_en("😀 abcd", 5, con_rango(3, 7, "xy")), "😀 xy");
    }

    #[test]
    fn lo_escrito_mientras_llegaba_la_respuesta_tambien_se_reemplaza() {
        let mut ed = Editor::open(None).expect("editor vacío");
        ed.rope = ropey::Rope::from_str("a.pu();");
        ed.cursor = editor::Position { line: 0, col: 4 };
        let (trigger, prefix) = ed.identifier_prefix_before_cursor();
        // Se pide en "pu|", y antes de que llegue la respuesta se escribe "s".
        ed.insert_char('s');
        let mut entry = completion_entry_from(&con_rango(2, 4, "push")).unwrap();
        entry.reemplazo = entry
            .lsp_range
            .and_then(|r| reemplazo_de(&ed, (trigger.line, trigger.col), &prefix, r));
        aceptar_sugerencia(&mut ed, &entry, trigger);
        assert_eq!(ed.rope.to_string(), "a.push();");
    }
}

#[cfg(test)]
mod tests_lsp_definicion_y_renombre {
    //! Lo que se prueba acá es el traductor entre lo que manda un servidor
    //! LSP y lo que hace Flint. Es donde están los errores de verdad: el
    //! protocolo admite tres formas de contestar dónde está una definición y
    //! dos de contestar un renombre, y las columnas vienen en UTF-16.
    use super::*;
    use serde_json::json;

    fn editor_con(texto: &str) -> Editor {
        let mut e = Editor::open(None).expect("editor vacío");
        e.rope = ropey::Rope::from_str(texto);
        e
    }

    #[test]
    fn entiende_las_tres_formas_de_contestar_una_definicion() {
        // Un Location suelto.
        let uno = json!({"uri": "file:///a.rs", "range": {"start": {"line": 3, "character": 7}, "end": {"line": 3, "character": 9}}});
        assert_eq!(
            primer_destino(&uno),
            Some(("file:///a.rs".to_string(), (3, 7), 1))
        );

        // Una lista de Location: se toma el primero y se cuentan todos.
        let lista = json!([uno, {"uri": "file:///b.rs", "range": {"start": {"line": 9, "character": 0}, "end": {"line": 9, "character": 1}}}]);
        assert_eq!(
            primer_destino(&lista),
            Some(("file:///a.rs".to_string(), (3, 7), 2))
        );

        // Una lista de LocationLink: gana targetSelectionRange (el nombre)
        // sobre targetRange (el cuerpo entero), que es donde uno quiere que
        // caiga el cursor.
        let link = json!([{
            "targetUri": "file:///c.rs",
            "targetRange": {"start": {"line": 10, "character": 0}, "end": {"line": 20, "character": 0}},
            "targetSelectionRange": {"start": {"line": 10, "character": 4}, "end": {"line": 10, "character": 8}}
        }]);
        assert_eq!(
            primer_destino(&link),
            Some(("file:///c.rs".to_string(), (10, 4), 1))
        );

        // Y lo que no es ninguna de las tres.
        assert_eq!(primer_destino(&Value::Null), None);
        assert_eq!(primer_destino(&json!([])), None);
    }

    #[test]
    fn la_uri_vuelve_a_ser_una_ruta() {
        assert_eq!(
            ruta_de_uri("file:///home/x/main.rs"),
            Some(PathBuf::from("/home/x/main.rs"))
        );
        // Con caracteres escapados, que es como salen los espacios y los
        // acentos de `file_uri`.
        assert_eq!(
            ruta_de_uri("file:///home/x/mis%20cosas/a%C3%B1o.rs"),
            Some(PathBuf::from("/home/x/mis cosas/año.rs"))
        );
        assert_eq!(ruta_de_uri("http://ejemplo.cl/a.rs"), None);
        // Ida y vuelta contra el generador de URIs de Flint.
        let p = PathBuf::from("/tmp/con espacio/ñ.rs");
        assert_eq!(ruta_de_uri(&lsp::file_uri(&p)), Some(p));
    }

    #[test]
    fn entiende_las_dos_formas_de_contestar_un_renombre() {
        let edicion = json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "newText": "z"});

        let viejo = json!({"changes": {"file:///a.rs": [edicion]}});
        let c = cambios_de_workspace_edit(&viejo).expect("se entiende");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, "file:///a.rs");

        let nuevo = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///b.rs", "version": 3}, "edits": [edicion]}
        ]});
        let c = cambios_de_workspace_edit(&nuevo).expect("se entiende");
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].0, "file:///b.rs");
    }

    #[test]
    fn un_renombre_que_pide_tocar_archivos_se_rechaza_entero() {
        // Crear o borrar archivos Flint no lo hace. Aplicar la mitad del
        // renombre y saltearse esa parte dejaría el proyecto sin compilar
        // sin que nadie lo avise, así que se corta antes de tocar nada.
        let con_archivos = json!({"documentChanges": [
            {"kind": "create", "uri": "file:///nuevo.rs"},
            {"textDocument": {"uri": "file:///b.rs"}, "edits": []}
        ]});
        assert!(cambios_de_workspace_edit(&con_archivos).is_err());
    }

    #[test]
    fn aplicar_varias_ediciones_no_las_desalinea() {
        // Todas las ediciones vienen contra el documento original. Si se
        // aplicaran de arriba para abajo, la segunda caería corrida por lo
        // que cambió de largo la primera.
        let mut ed = editor_con("uno dos uno\nuno\n");
        let ediciones = vec![
            json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 3}}, "newText": "TRESMIL"}),
            json!({"range": {"start": {"line": 0, "character": 8}, "end": {"line": 0, "character": 11}}, "newText": "TRESMIL"}),
            json!({"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 3}}, "newText": "TRESMIL"}),
        ];
        assert!(aplicar_ediciones(&mut ed, &ediciones));
        assert_eq!(ed.rope.to_string(), "TRESMIL dos TRESMIL\nTRESMIL\n");
    }

    #[test]
    fn un_renombre_entero_se_deshace_de_una() {
        // Tres ocurrencias en el archivo, un solo Ctrl+Z.
        let mut ed = editor_con("a a a\n");
        let ediciones: Vec<Value> = (0..3)
            .map(|i| {
                let c = i * 2;
                json!({"range": {"start": {"line": 0, "character": c}, "end": {"line": 0, "character": c + 1}}, "newText": "bb"})
            })
            .collect();
        assert!(aplicar_ediciones(&mut ed, &ediciones));
        assert_eq!(ed.rope.to_string(), "bb bb bb\n");
        assert!(ed.undo());
        assert_eq!(ed.rope.to_string(), "a a a\n");
    }

    #[test]
    fn las_columnas_del_servidor_vienen_en_utf16() {
        // Un emoji ocupa dos unidades UTF-16 y un solo carácter. Si se
        // tomaran las columnas como caracteres, el reemplazo caería corrido.
        let mut ed = editor_con("let 🌞x = 1;\n");
        let ediciones = vec![json!({
            "range": {"start": {"line": 0, "character": 6}, "end": {"line": 0, "character": 7}},
            "newText": "y"
        })];
        assert!(aplicar_ediciones(&mut ed, &ediciones));
        assert_eq!(ed.rope.to_string(), "let 🌞y = 1;\n");
    }

    #[test]
    fn una_lista_de_ediciones_vacia_no_gasta_un_paso_de_deshacer() {
        let mut ed = editor_con("hola\n");
        assert!(!aplicar_ediciones(&mut ed, &[]));
        assert!(!ed.dirty, "no tendría que haber marcado el buffer como sucio");
        assert!(!ed.undo(), "no tendría que haber nada que deshacer");
    }

    #[test]
    fn las_capacidades_se_leen_en_sus_dos_formas() {
        // `true` a secas y un objeto con opciones significan lo mismo.
        let a = json!({"capabilities": {"definitionProvider": true, "renameProvider": {"prepareProvider": true}}});
        let c = Capacidades::de(&a);
        assert!(c.definition && c.rename);

        // `false` y ausente significan que no.
        let b = json!({"capabilities": {"definitionProvider": false}});
        let c = Capacidades::de(&b);
        assert!(!c.definition && !c.rename && !c.hover);
    }

    #[test]
    fn el_reemplazo_en_el_proyecto_cae_en_la_linea_y_columna_justas() {
        let mut ed = editor_con("ñandú = uno;\n😀 uno(uno)\n");
        let re = busqueda::expresion("uno", false).unwrap();
        let cambios = cambios_de_reemplazo(&ed, &re, "dos", false);
        let pos = |l, c| editor::Position { line: l, col: c };
        assert_eq!(
            cambios.iter().map(|(a, b, _)| (*a, *b)).collect::<Vec<_>>(),
            vec![(pos(0, 8), pos(0, 11)), (pos(1, 2), pos(1, 5)), (pos(1, 6), pos(1, 9))]
        );
        ed.replace_ranges(&cambios);
        assert_eq!(ed.rope.to_string(), "ñandú = dos;\n😀 dos(dos)\n");
        // Un solo paso de deshacer para todo el archivo.
        ed.undo();
        assert_eq!(ed.rope.to_string(), "ñandú = uno;\n😀 uno(uno)\n");
    }

    #[test]
    fn entiende_las_formas_de_contestar_una_firma() {
        // rust-analyzer: parámetros como posiciones UTF-16 y activeParameter
        // en la respuesta.
        let r = json!({
            "signatures": [{
                "label": "fn push(&mut self, value: T)",
                "parameters": [{"label": [8, 17]}, {"label": [19, 27]}]
            }],
            "activeSignature": 0,
            "activeParameter": 1
        });
        let f = firma_de(&r).unwrap();
        assert_eq!(&f.etiqueta.chars().collect::<Vec<_>>()[19..27].iter().collect::<String>(), "value: T");
        assert_eq!(f.parametro, Some((19, 27)));
        assert_eq!((f.indice, f.total), (0, 1));

        // pylsp: parámetros como texto, que se busca después del paréntesis
        // aunque el nombre de la función lo contenga.
        let r = json!({
            "signatures": [
                {"label": "a(x)", "parameters": [{"label": "x"}]},
                {"label": "x(a, x)", "parameters": [{"label": "a"}, {"label": "x"}], "activeParameter": 1}
            ],
            "activeSignature": 1
        });
        let f = firma_de(&r).unwrap();
        assert_eq!(f.parametro, Some((5, 6)));
        assert_eq!((f.indice, f.total), (1, 2));

        // Sin activeParameter vale el primero; con null, ninguno.
        let sin = json!({"signatures": [{"label": "f(a, b)", "parameters": [{"label": "a"}, {"label": "b"}]}]});
        assert_eq!(firma_de(&sin).unwrap().parametro, Some((2, 3)));
        let nulo = json!({"signatures": [{"label": "f(a)", "parameters": [{"label": "a"}], "activeParameter": null}]});
        assert_eq!(firma_de(&nulo).unwrap().parametro, None);

        // Posiciones UTF-16 con un emoji antes: 2 unidades, 1 carácter.
        let emoji = json!({"signatures": [{"label": "f(😀, b)", "parameters": [{"label": [2, 4]}, {"label": [6, 7]}], "activeParameter": 1}]});
        assert_eq!(firma_de(&emoji).unwrap().parametro, Some((5, 6)));

        // Sin firmas: el cursor salió de la llamada.
        assert_eq!(firma_de(&json!({"signatures": []})), None);
        assert_eq!(firma_de(&json!(null)), None);
    }

    #[test]
    fn entiende_las_formas_de_contestar_un_hover() {
        // MarkupContent, la forma actual (rust-analyzer, pylsp).
        let r = json!({"contents": {"kind": "markdown", "value": "```rust\nfn main()\n```"}});
        assert_eq!(contenido_de_hover(&r), Some(("```rust\nfn main()\n```".to_string(), true)));
        let r = json!({"contents": {"kind": "plaintext", "value": "  int x  "}});
        assert_eq!(contenido_de_hover(&r), Some(("int x".to_string(), false)));

        // MarkedString suelto: string o bloque de código con su lenguaje.
        let r = json!({"contents": "**hola**"});
        assert_eq!(contenido_de_hover(&r), Some(("**hola**".to_string(), true)));
        let r = json!({"contents": {"language": "python", "value": "def f()"}});
        assert_eq!(contenido_de_hover(&r), Some(("```python\ndef f()\n```".to_string(), true)));

        // Lista de MarkedString: se juntan como párrafos.
        let r = json!({"contents": [{"language": "go", "value": "func F()"}, "Hace algo."]});
        assert_eq!(
            contenido_de_hover(&r),
            Some(("```go\nfunc F()\n```\n\nHace algo.".to_string(), true))
        );

        // Vacío o sin contenido: nada que mostrar.
        assert_eq!(contenido_de_hover(&json!({"contents": ""})), None);
        assert_eq!(contenido_de_hover(&json!({"contents": []})), None);
        assert_eq!(contenido_de_hover(&json!(null)), None);
    }

    #[test]
    fn el_texto_plano_del_hover_se_parte_al_ancho() {
        let lineas = envolver_texto_plano("abcdefghij\ncort", 4);
        let textos: Vec<String> = lineas.iter().map(|l| l.to_string()).collect();
        assert_eq!(textos, vec!["abcd", "efgh", "ij", "cort"]);
    }
}

#[cfg(test)]
mod tests_lsp_de_punta_a_punta {
    //! El cliente LSP entero, contra un servidor de verdad por stdio: el de
    //! mentira de `examples/lsp_falso.rs`, que `cargo test` compila solo.
    //! Cada test arranca su propio servidor sobre su propio archivo y maneja
    //! Flint con las mismas funciones que usa el teclado.
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Prueba {
        app: App,
        espejo: PathBuf,
        dir: PathBuf,
    }

    impl Drop for Prueba {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// `target/debug/examples/lsp_falso`, al lado del directorio del
    /// ejecutable de los tests (`target/debug/deps/`).
    fn servidor_falso() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let debug = exe.parent().unwrap().parent().unwrap();
        let ruta = debug.join("examples").join("lsp_falso");
        assert!(ruta.exists(), "falta {}; lo compila `cargo test`", ruta.display());
        ruta
    }

    fn abrir(texto: &str) -> Prueba {
        let mut p = arrancar(texto, servidor_falso(), None);
        esperar(&mut p, "el arranque", |p| p.app.lsp.is_some());
        // El servidor escribe el espejo al recibir el didOpen.
        esperar(&mut p, "el didOpen", |p| std::fs::read_to_string(&p.espejo).is_ok_and(|t| t == texto));
        p
    }

    /// Lanza `servidor` sobre un archivo con `texto` y devuelve sin esperar
    /// a que conteste, como al arrancar Flint. `args` reemplaza al archivo
    /// espejo que recibe el servidor falso.
    fn arrancar(texto: &str, servidor: PathBuf, args: Option<Vec<String>>) -> Prueba {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "flint-lsp-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let archivo = dir.join("a.rs");
        std::fs::write(&archivo, texto).unwrap();
        let espejo = dir.join("espejo.txt");

        let buffer = Buffer::open(Some(archivo.clone())).unwrap();
        let args = args.unwrap_or_else(|| vec![espejo.display().to_string()]);
        let client = lsp::LspClient::spawn(servidor.to_str().unwrap(), &args).unwrap();
        let root = lsp::file_uri(&dir);
        let mut app = app_de_prueba(buffer);
        app.lsp_arrancando = Some(lanzar_arranque(client, &root, "rust", "lsp_falso").unwrap());
        Prueba { app, espejo, dir }
    }

    /// Atiende lo que mande el servidor hasta que `listo` se cumpla, o falla
    /// a los 5 s diciendo qué se esperaba.
    fn esperar(p: &mut Prueba, que: &str, listo: impl Fn(&Prueba) -> bool) {
        let hasta = Instant::now() + Duration::from_secs(5);
        while !listo(p) {
            assert!(Instant::now() < hasta, "no llegó {que}; estado: {}", p.app.buffers[0].ed.status);
            if p.app.lsp_arrancando.is_some() {
                atender_arranque_lsp(&mut p.app);
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let msg = p.app.lsp.as_ref().and_then(|c| c.recv_timeout(Duration::from_millis(20)));
            if let Some(msg) = msg {
                handle_lsp_message(&mut p.app, msg);
            }
        }
    }

    fn tipear(p: &mut Prueba, texto: &str) {
        for c in texto.chars() {
            let accion = if c == '\n' { Action::InsertNewline } else { Action::InsertChar(c) };
            dispatch_action(&mut p.app, accion, 10);
        }
    }

    fn tecla(p: &mut Prueba, code: KeyCode) {
        handle_key(&mut p.app, KeyEvent::new(code, KeyModifiers::NONE), 10);
    }

    fn texto(p: &Prueba) -> String {
        p.app.buffers[0].ed.rope.to_string()
    }

    fn ed(p: &mut Prueba) -> &mut Editor {
        &mut p.app.buffers[0].ed
    }

    /// Manda lo pendiente y espera a que la copia del servidor sea igual a
    /// la del editor.
    fn sincronizar(p: &mut Prueba) {
        sync_lsp_ahora(&mut p.app);
        let esperado = texto(p);
        esperar(p, "la sincronización", |p| {
            std::fs::read_to_string(&p.espejo).is_ok_and(|t| t == esperado)
        });
    }

    #[test]
    fn el_servidor_tiene_el_mismo_texto_despues_de_cualquier_edicion() {
        let mut p = abrir("fn main() {\n    let x = 1;\n}\n");
        assert!(p.app.lsp_incremental, "el servidor falso anuncia sync incremental");

        // Tipeo con acentos y emoji (dos unidades UTF-16), Enter con
        // sangría automática, y Backspace.
        ed(&mut p).cursor = editor::Position { line: 1, col: 14 };
        tipear(&mut p, " // ñandú 😀 fin\nlet y = 2;");
        dispatch_action(&mut p.app, Action::Backspace, 10);
        sincronizar(&mut p);

        // Varios cursores escribiendo a la vez.
        ed(&mut p).apply_selections(vec![
            editor::Selection { anchor: editor::Position { line: 0, col: 0 }, cursor: editor::Position { line: 0, col: 0 } },
            editor::Selection { anchor: editor::Position { line: 1, col: 4 }, cursor: editor::Position { line: 1, col: 4 } },
        ]);
        tipear(&mut p, "#");
        sincronizar(&mut p);

        // Deshacer y rehacer.
        dispatch_action(&mut p.app, Action::Undo, 10);
        dispatch_action(&mut p.app, Action::Undo, 10);
        sincronizar(&mut p);
        dispatch_action(&mut p.app, Action::Redo, 10);
        sincronizar(&mut p);

        // Una selección de varias líneas reemplazada por una letra, y un
        // bloque indentado con Tab.
        ed(&mut p).apply_selections(vec![editor::Selection {
            anchor: editor::Position { line: 0, col: 3 },
            cursor: editor::Position { line: 2, col: 2 },
        }]);
        tipear(&mut p, "z");
        sincronizar(&mut p);
        ed(&mut p).apply_selections(vec![editor::Selection {
            anchor: editor::Position { line: 0, col: 0 },
            cursor: editor::Position { line: 1, col: 0 },
        }]);
        dispatch_action(&mut p.app, Action::InsertTab, 10);
        sincronizar(&mut p);
    }

    #[test]
    fn se_puede_escribir_mientras_el_servidor_arranca() {
        let mut p = arrancar("fn main() {}\n", servidor_falso(), None);
        // Nada de esperar: se escribe antes de que el servidor conteste.
        assert!(p.app.lsp.is_none() && p.app.lsp_arrancando.is_some());
        ed(&mut p).cursor = editor::Position { line: 0, col: 12 };
        tipear(&mut p, " // escrito antes");
        esperar(&mut p, "el arranque", |p| p.app.lsp.is_some());
        assert!(p.app.buffers[0].ed.status.contains("lsp_falso listo"), "{}", p.app.buffers[0].ed.status);
        // Lo escrito antes llega en el didOpen, una sola vez: si también se
        // mandara como cambio, la copia del servidor lo tendría repetido.
        sincronizar(&mut p);
        tipear(&mut p, "!");
        sincronizar(&mut p);
        assert_eq!(texto(&p), "fn main() {} // escrito antes!\n");
    }

    #[test]
    fn un_servidor_que_no_contesta_no_deja_esperando() {
        let mut p = arrancar("x\n", PathBuf::from("sleep"), Some(vec!["30".to_string()]));
        p.app.lsp_arrancando.as_mut().unwrap().limite = Instant::now() + Duration::from_millis(100);
        esperar(&mut p, "el aviso", |p| p.app.lsp_arrancando.is_none());
        assert!(p.app.lsp.is_none());
        assert!(p.app.buffers[0].ed.status.contains("no respondió"), "{}", p.app.buffers[0].ed.status);
    }

    #[test]
    fn un_servidor_que_se_cae_al_arrancar_se_avisa() {
        let mut p = arrancar("x\n", PathBuf::from("true"), Some(Vec::new()));
        esperar(&mut p, "el aviso", |p| p.app.lsp_arrancando.is_none());
        assert!(p.app.lsp.is_none());
        assert!(p.app.buffers[0].ed.status.contains("se cerró"), "{}", p.app.buffers[0].ed.status);
    }

    #[test]
    fn los_diagnosticos_llegan_con_la_columna_en_caracteres() {
        let mut p = abrir("fn main() {}\n");
        ed(&mut p).cursor = editor::Position { line: 0, col: 12 };
        // El emoji ocupa dos unidades UTF-16: sin convertir, el subrayado
        // quedaría corrido un lugar.
        tipear(&mut p, " // 😀 ERROR");
        sync_lsp_ahora(&mut p.app);
        esperar(&mut p, "el diagnóstico", |p| !p.app.buffers[0].ed.diagnostics.is_empty());
        let d = &p.app.buffers[0].ed.diagnostics[0];
        assert_eq!((d.line, d.start_col, d.end_col), (0, 18, 23));

        // Al borrarlo, el servidor manda la lista vacía y se van.
        for _ in 0..5 {
            dispatch_action(&mut p.app, Action::Backspace, 10);
        }
        sync_lsp_ahora(&mut p.app);
        esperar(&mut p, "la lista vacía", |p| p.app.buffers[0].ed.diagnostics.is_empty());
    }

    #[test]
    fn autocompletar_reemplaza_lo_que_dice_el_servidor() {
        let mut p = abrir("fn main() {\n    v.puxx;\n}\n");
        ed(&mut p).cursor = editor::Position { line: 1, col: 8 };
        dispatch_action(&mut p.app, Action::TriggerCompletion, 10);
        esperar(&mut p, "la lista", |p| matches!(p.app.buffers[0].ed.mode, Mode::Completion { .. }));
        tecla(&mut p, KeyCode::Enter);
        // El rango del servidor abarca "puxx" entero, no solo lo de antes
        // del cursor.
        assert_eq!(texto(&p), "fn main() {\n    v.push;\n}\n");
        sincronizar(&mut p);
    }

    #[test]
    fn hover_definicion_y_vuelta_atras() {
        let mut p = abrir("fn main() {\n    main();\n}\n");
        ed(&mut p).cursor = editor::Position { line: 1, col: 5 };
        dispatch_action(&mut p.app, Action::Hover, 10);
        esperar(&mut p, "el hover", |p| matches!(p.app.buffers[0].ed.mode, Mode::Hover { .. }));
        let hover: String = p.app.hover_lines.iter().map(|l| l.to_string()).collect();
        assert!(hover.contains("fn falso()"), "{hover}");
        tecla(&mut p, KeyCode::Esc);

        dispatch_action(&mut p.app, Action::GotoDefinition, 10);
        esperar(&mut p, "el salto", |p| p.app.buffers[0].ed.cursor.line == 0);
        assert_eq!(p.app.buffers[0].ed.cursor, editor::Position { line: 0, col: 3 });
        dispatch_action(&mut p.app, Action::JumpBack, 10);
        assert_eq!(p.app.buffers[0].ed.cursor, editor::Position { line: 1, col: 5 });
    }

    #[test]
    fn renombrar_cambia_todas_las_apariciones_y_se_deshace_de_una_vez() {
        let mut p = abrir("fn uno() {}\nfn main() { uno(); uno(); }\n");
        ed(&mut p).cursor = editor::Position { line: 0, col: 4 };
        dispatch_action(&mut p.app, Action::RenamePrompt, 10);
        // El aviso viene con el nombre actual escrito, para editarlo.
        for _ in 0..3 {
            tecla(&mut p, KeyCode::Backspace);
        }
        for c in "dos".chars() {
            tecla(&mut p, KeyCode::Char(c));
        }
        tecla(&mut p, KeyCode::Enter);
        esperar(&mut p, "el renombre", |p| p.app.buffers[0].ed.rope.to_string().contains("dos"));
        assert_eq!(texto(&p), "fn dos() {}\nfn main() { dos(); dos(); }\n");
        sincronizar(&mut p);
        dispatch_action(&mut p.app, Action::Undo, 10);
        assert_eq!(texto(&p), "fn uno() {}\nfn main() { uno(); uno(); }\n");
        sincronizar(&mut p);
    }

    #[test]
    fn la_ayuda_de_firmas_sigue_al_parametro_y_se_cierra_sola() {
        let mut p = abrir("fn main() {\n    \n}\n");
        ed(&mut p).cursor = editor::Position { line: 1, col: 4 };
        let parametro = |p: &Prueba| p.app.firma.as_ref().and_then(|f| f.firma.parametro);

        tipear(&mut p, "suma(");
        esperar(&mut p, "la firma", |p| parametro(p) == Some((8, 14)));
        tipear(&mut p, "1,");
        esperar(&mut p, "el segundo parámetro", |p| parametro(p) == Some((16, 22)));
        // El cierre automático ya puso el `)`: escribirlo pasa por encima.
        tipear(&mut p, " 2)");
        esperar(&mut p, "que se cierre", |p| p.app.firma.is_none());
        assert_eq!(texto(&p), "fn main() {\n    suma(1, 2)\n}\n");

        // Esc también la cierra.
        tipear(&mut p, ";\nsuma(");
        esperar(&mut p, "la firma otra vez", |p| p.app.firma.is_some());
        dispatch_action(&mut p.app, Action::EscapeKey, 10);
        assert!(p.app.firma.is_none());
    }

    #[test]
    fn formatear_aplica_lo_que_manda_el_servidor() {
        let mut p = abrir("fn main() {   \n    x();  \n}\n");
        dispatch_action(&mut p.app, Action::Format, 10);
        assert_eq!(texto(&p), "fn main() {\n    x();\n}\n");
        sincronizar(&mut p);
    }
}

#[cfg(test)]
mod tests_eventos_de_plugins {
    //! Los eventos de plugin de punta a punta: un script de verdad, cargado
    //! desde disco, sobre archivos de verdad.
    use super::*;

    const SCRIPT: &str = r#"
        flint.on("open", function() flint.status("abierto " .. flint.filename()) end)

        -- Antes de guardar: que el archivo termine en salto de línea.
        flint.on("before_save", function()
          if flint.text():sub(-1) ~= "\n" then
            local n = flint.line_count()
            flint.set_cursor(n, #flint.line(n) + 1)
            flint.insert_text("\n")
          end
        end)

        -- Al guardar: una marca y guardar otra vez. Ese segundo guardado no
        -- tiene que volver a disparar el evento.
        flint.on("save", function()
          flint.insert_text("x")
          flint.action("save")
        end)
    "#;

    fn preparar(nombre: &str) -> (App, PathBuf) {
        let dir = std::env::temp_dir().join(format!("flint-eventos-{}-{nombre}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins")).unwrap();
        std::fs::write(dir.join("plugins/eventos.lua"), SCRIPT).unwrap();
        std::fs::write(dir.join("a.txt"), "hola").unwrap();
        std::fs::write(dir.join("b.txt"), "otro\n").unwrap();
        let mut app = app_de_prueba(Buffer::open(Some(dir.join("a.txt"))).unwrap());
        let carga = plugins::PluginBridge::load(&[dir.join("plugins")]);
        assert!(carga.errors.is_empty(), "{:?}", carga.errors);
        app.plugins = Some(carga.bridge);
        (app, dir)
    }

    #[test]
    fn abrir_avisa_con_el_archivo_que_se_abrio() {
        let (mut app, dir) = preparar("abrir");
        emitir_evento(&mut app, plugins::Evento::Abrir);
        assert!(app.buffers[0].ed.status.ends_with("a.txt"), "{}", app.buffers[0].ed.status);
        open_file_into_new_buffer(&mut app, dir.join("b.txt").display().to_string());
        assert_eq!(app.active, 1);
        assert!(app.buffers[1].ed.status.ends_with("b.txt"), "{}", app.buffers[1].ed.status);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn antes_de_guardar_cambia_lo_que_se_escribe_y_guardar_no_se_repite() {
        let (mut app, dir) = preparar("guardar");
        dispatch_action(&mut app, Action::Save, 10);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hola\n");

        avisar_guardados(&mut app);
        // El manejador puso su marca y guardó: una sola vez, aunque ese
        // guardado también marcó el buffer como recién guardado.
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hola\nx");
        avisar_guardados(&mut app);
        avisar_guardados(&mut app);
        assert_eq!(app.buffers[0].ed.rope.to_string(), "hola\nx");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sin_manejadores_no_se_arma_la_foto_ni_pasa_nada() {
        let (mut app, dir) = preparar("sin");
        app.plugins = Some(plugins::PluginBridge::load(&[]).bridge);
        dispatch_action(&mut app, Action::Save, 10);
        avisar_guardados(&mut app);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hola");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
