use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;

use mlua::{Function, Lua, RegistryKey};

/// Lo que un script Lua registró: `id`/`label` para mostrarlo en la paleta de
/// comandos, y la función real guardada en el registro de Lua para poder
/// llamarla más tarde.
pub struct PluginCommand {
    pub id: String,
    pub label: String,
    key: RegistryKey,
}

/// Una tecla que un plugin pidió atar, todavía sin resolver: `destino` puede
/// ser el id de un comando suyo o el nombre de una acción de Flint. Se
/// resuelve después de cargar todos los scripts, cuando ya se sabe qué
/// comandos existen.
pub struct PluginBind {
    pub tecla: String,
    pub destino: String,
    /// `true` para la capa modal NORMAL, `false` para la capa directa.
    pub modal: bool,
}

/// Lo que un comando de plugin ve del editor cuando se lo invoca. Es una
/// foto, no una referencia viva: el script corre con el buffer quieto y lo
/// que pida cambiar sale como `PluginEffect`, que Flint aplica después. Así
/// un plugin no puede dejar el editor a medio camino si falla en el medio.
pub struct PluginContext {
    pub filename: String,
    /// El identificador del lenguaje (`rust`, `python`…), vacío si no hay.
    pub language: String,
    pub line_count: usize,
    /// Línea y columna del cursor primario, contando desde 1 — como las
    /// muestra la barra de estado y como las pide "Ir a la línea".
    pub cursor_line: usize,
    pub cursor_col: usize,
    /// El texto seleccionado, vacío si no hay selección.
    pub selection: String,
    /// El contenido completo del buffer.
    pub text: String,
    /// Lo que hay para pegar.
    pub clipboard: String,
}

/// Lo que un comando de plugin pidió que pase, en el orden en que lo pidió.
/// Flint los aplica después de que el script terminó.
pub enum PluginEffect {
    /// Mostrar un texto en la barra de estado.
    Status(String),
    /// Insertar texto en cada cursor.
    InsertText(String),
    /// Reemplazar lo seleccionado (o insertar, si no hay selección).
    ReplaceSelection(String),
    /// Dejar un texto en el portapapeles.
    SetClipboard(String),
    /// Mover el cursor primario. Línea y columna desde 1.
    SetCursor { line: usize, col: usize },
    /// Ejecutar una acción de Flint por su nombre estable, el mismo que
    /// lista `flint --actions`.
    Action(String),
}

/// El puente entre Flint y Lua.
///
/// Un script ve una sola tabla global, `flint`. Al cargarse puede registrar
/// comandos (`register_command`) y atar teclas (`bind`); adentro de un
/// comando puede leer el estado del editor y pedir cambios. Lo que no puede
/// es tocar el buffer directamente: todo lo que cambia sale como una lista
/// de `PluginEffect` que Flint aplica cuando el script terminó.
pub struct PluginBridge {
    lua: Lua,
    efectos: Rc<RefCell<Vec<PluginEffect>>>,
    ctx: Rc<RefCell<PluginContext>>,
}

impl Default for PluginContext {
    fn default() -> Self {
        PluginContext {
            filename: String::new(),
            language: String::new(),
            line_count: 0,
            cursor_line: 1,
            cursor_col: 1,
            selection: String::new(),
            text: String::new(),
            clipboard: String::new(),
        }
    }
}

/// Dónde busca Flint los plugins, de mayor a menor precedencia:
///
/// 1. `./plugins` — relativo a donde se arranca; es lo que hace falta para
///    desarrollar dentro del repo, y era el único lugar que se miraba antes.
/// 2. `~/.config/flint/plugins` — los tuyos, junto al `theme.toml` que ya
///    vivía ahí.
/// 3. `/usr/share/flint/plugins` — los que instala el paquete `.deb`.
///
/// Mirar solo el primero dejaba los plugins de ejemplo del `.deb` en un
/// directorio que nadie leía: instalado, Flint nunca cargaba ninguno salvo
/// que lo arrancaras parado justo en un directorio con `./plugins`.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("plugins")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config/flint/plugins"));
    }
    dirs.push(PathBuf::from("/usr/share/flint/plugins"));
    dirs
}

/// Lo que dejó la carga de los plugins.
pub struct PluginLoad {
    pub bridge: PluginBridge,
    pub commands: Vec<PluginCommand>,
    pub binds: Vec<PluginBind>,
    pub errors: Vec<String>,
}

impl PluginBridge {
    /// Carga todos los `*.lua` de `dirs` (los que existan). Un script con
    /// errores no aborta a los demás: su error se junta y se avisa.
    pub fn load(dirs: &[PathBuf]) -> PluginLoad {
        let lua = Lua::new();
        let efectos: Rc<RefCell<Vec<PluginEffect>>> = Rc::new(RefCell::new(Vec::new()));
        let ctx = Rc::new(RefCell::new(PluginContext::default()));
        let registered: Rc<RefCell<Vec<(String, String, RegistryKey)>>> =
            Rc::new(RefCell::new(Vec::new()));
        let binds: Rc<RefCell<Vec<PluginBind>>> = Rc::new(RefCell::new(Vec::new()));

        let mut errors = Vec::new();
        if let Err(e) = install_api(&lua, &registered, &binds, &efectos, &ctx) {
            errors.push(format!("No se pudo preparar la API de plugins: {e}"));
        }

        // Un mismo nombre de archivo en dos directorios se carga una sola
        // vez, la del directorio de mayor precedencia: así un plugin propio
        // reemplaza al del sistema en vez de que corran los dos y la paleta
        // termine con el comando duplicado.
        let mut seen: Vec<OsString> = Vec::new();
        for dir in dirs {
            if !dir.is_dir() {
                continue;
            }
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
                    paths.sort();
                    for path in paths {
                        if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                            continue;
                        }
                        let Some(file_name) = path.file_name().map(OsString::from) else {
                            continue;
                        };
                        if seen.contains(&file_name) {
                            continue;
                        }
                        seen.push(file_name);
                        match std::fs::read_to_string(&path) {
                            Ok(src) => {
                                let name = path.display().to_string();
                                if let Err(e) = lua.load(&src).set_name(&name).exec() {
                                    errors.push(format!("{name}: {e}"));
                                }
                            }
                            Err(e) => errors.push(format!("{}: {e}", path.display())),
                        }
                    }
                }
                Err(e) => errors.push(format!("No se pudo leer {}: {e}", dir.display())),
            }
        }

        // No se puede usar `Rc::try_unwrap` acá: la función de Lua que
        // `register_command` guardó en el registro conserva su propio clon
        // del `Rc` mientras `lua` siga viva (que es justo lo que queremos,
        // para poder invocar esos comandos más tarde) — así que el conteo
        // nunca baja a 1. En cambio, se vacía el `Vec` de adentro del
        // `RefCell` y se deja uno vacío en su lugar, sin tocar el `Rc`.
        let commands: Vec<PluginCommand> = std::mem::take(&mut *registered.borrow_mut())
            .into_iter()
            .map(|(id, label, key)| PluginCommand { id, label, key })
            .collect();
        let binds = std::mem::take(&mut *binds.borrow_mut());

        PluginLoad {
            bridge: PluginBridge { lua, efectos, ctx },
            commands,
            binds,
            errors,
        }
    }

    /// Ejecuta el comando de un plugin con `ctx` como la foto del editor que
    /// va a ver, y devuelve lo que pidió que pase, en orden.
    pub fn invoke(
        &self,
        cmd: &PluginCommand,
        ctx: PluginContext,
    ) -> Result<Vec<PluginEffect>, String> {
        *self.ctx.borrow_mut() = ctx;
        self.efectos.borrow_mut().clear();

        let func: Function = self
            .lua
            .registry_value(&cmd.key)
            .map_err(|e| e.to_string())?;
        let resultado = func.call::<()>(()).map_err(|e| e.to_string());
        let efectos = self.efectos.borrow_mut().drain(..).collect();
        // Si el script falló a la mitad se descarta todo lo que había pedido
        // hasta ahí: aplicar media intención es peor que no aplicar ninguna.
        resultado?;
        Ok(efectos)
    }
}

#[allow(clippy::type_complexity)]
fn install_api(
    lua: &Lua,
    registered: &Rc<RefCell<Vec<(String, String, RegistryKey)>>>,
    binds: &Rc<RefCell<Vec<PluginBind>>>,
    efectos: &Rc<RefCell<Vec<PluginEffect>>>,
    ctx: &Rc<RefCell<PluginContext>>,
) -> mlua::Result<()> {
    let flint = lua.create_table()?;

    // ---------- registro, en tiempo de carga ----------

    let reg = registered.clone();
    let register_command =
        lua.create_function(move |lua_ctx, (id, label, f): (String, String, Function)| {
            let key = lua_ctx.create_registry_value(f)?;
            reg.borrow_mut().push((id, label, key));
            Ok(())
        })?;
    flint.set("register_command", register_command)?;

    // `flint.bind("ctrl+j", "save")` o `flint.bind("ctrl+j", "mi_comando")`.
    // El tercer argumento, opcional, dice si la tecla va en la capa modal
    // NORMAL en vez de la directa.
    let bind_cell = binds.clone();
    let bind = lua.create_function(
        move |_, (tecla, destino, modal): (String, String, Option<bool>)| {
            bind_cell.borrow_mut().push(PluginBind {
                tecla,
                destino,
                modal: modal.unwrap_or(false),
            });
            Ok(())
        },
    )?;
    flint.set("bind", bind)?;

    // ---------- lectura, adentro de un comando ----------

    macro_rules! leer {
        ($nombre:literal, $f:expr) => {{
            let celda = ctx.clone();
            let f = lua.create_function(move |_, ()| {
                let c = celda.borrow();
                Ok($f(&*c))
            })?;
            flint.set($nombre, f)?;
        }};
    }

    leer!("filename", |c: &PluginContext| c.filename.clone());
    leer!("line_count", |c: &PluginContext| c.line_count);
    leer!("selection", |c: &PluginContext| c.selection.clone());
    leer!("text", |c: &PluginContext| c.text.clone());
    leer!("clipboard", |c: &PluginContext| c.clipboard.clone());

    // El lenguaje sale como `nil` cuando el archivo no tiene ninguno, que en
    // Lua es más natural que una cadena vacía para preguntar `if`.
    let celda = ctx.clone();
    let language = lua.create_function(move |_, ()| {
        let c = celda.borrow();
        Ok((!c.language.is_empty()).then(|| c.language.clone()))
    })?;
    flint.set("language", language)?;

    // Dos valores de vuelta, que es como Lua devuelve un par.
    let celda = ctx.clone();
    let cursor = lua.create_function(move |_, ()| {
        let c = celda.borrow();
        Ok((c.cursor_line, c.cursor_col))
    })?;
    flint.set("cursor", cursor)?;

    // `flint.line(n)` con `n` desde 1, igual que el resto de la API.
    let celda = ctx.clone();
    let line = lua.create_function(move |_, n: usize| {
        let c = celda.borrow();
        Ok(c.text
            .lines()
            .nth(n.saturating_sub(1))
            .unwrap_or_default()
            .to_string())
    })?;
    flint.set("line", line)?;

    // ---------- pedidos, adentro de un comando ----------

    macro_rules! pedir {
        ($nombre:literal, $variante:expr) => {{
            let celda = efectos.clone();
            let f = lua.create_function(move |_, texto: String| {
                celda.borrow_mut().push($variante(texto));
                Ok(())
            })?;
            flint.set($nombre, f)?;
        }};
    }

    pedir!("status", PluginEffect::Status);
    pedir!("insert_text", PluginEffect::InsertText);
    pedir!("replace_selection", PluginEffect::ReplaceSelection);
    pedir!("set_clipboard", PluginEffect::SetClipboard);
    pedir!("action", PluginEffect::Action);

    let celda = efectos.clone();
    let set_cursor = lua.create_function(move |_, (line, col): (usize, Option<usize>)| {
        celda.borrow_mut().push(PluginEffect::SetCursor {
            line,
            col: col.unwrap_or(1),
        });
        Ok(())
    })?;
    flint.set("set_cursor", set_cursor)?;

    lua.globals().set("flint", flint)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Escribe un script en un directorio propio y lo carga. El directorio
    /// lleva el pid en el nombre para que dos tests en paralelo no se pisen.
    fn cargar(nombre: &str, fuente: &str) -> (PluginLoad, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "flint-plugins-{}-{nombre}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear el directorio de prueba");
        std::fs::write(dir.join("p.lua"), fuente).expect("escribir el script");
        let carga = PluginBridge::load(std::slice::from_ref(&dir));
        (carga, dir)
    }

    fn limpiar(dir: PathBuf) {
        let _ = std::fs::remove_dir_all(dir);
    }

    fn ctx_de_prueba() -> PluginContext {
        PluginContext {
            filename: "/tmp/x.rs".to_string(),
            language: "rust".to_string(),
            line_count: 3,
            cursor_line: 2,
            cursor_col: 5,
            selection: "hola".to_string(),
            text: "uno\ndos\ntres\n".to_string(),
            clipboard: "pegable".to_string(),
        }
    }

    fn textos(efectos: &[PluginEffect]) -> Vec<String> {
        efectos
            .iter()
            .map(|e| match e {
                PluginEffect::Status(t) => format!("status:{t}"),
                PluginEffect::InsertText(t) => format!("insert:{t}"),
                PluginEffect::ReplaceSelection(t) => format!("replace:{t}"),
                PluginEffect::SetClipboard(t) => format!("clipboard:{t}"),
                PluginEffect::SetCursor { line, col } => format!("cursor:{line}:{col}"),
                PluginEffect::Action(t) => format!("action:{t}"),
            })
            .collect()
    }

    #[test]
    fn un_comando_puede_leer_todo_el_contexto() {
        let (carga, dir) = cargar(
            "leer",
            r#"
            flint.register_command("x", "X", function()
              local l, c = flint.cursor()
              flint.status(table.concat({
                flint.filename(), flint.language(), tostring(flint.line_count()),
                l .. ":" .. c, flint.selection(), flint.line(2),
                flint.clipboard(), tostring(#flint.text()),
              }, "|"))
            end)
            "#,
        );
        assert!(carga.errors.is_empty(), "{:?}", carga.errors);
        assert_eq!(carga.commands.len(), 1);
        let efectos = carga
            .bridge
            .invoke(&carga.commands[0], ctx_de_prueba())
            .expect("el script corre");
        assert_eq!(
            textos(&efectos),
            vec!["status:/tmp/x.rs|rust|3|2:5|hola|dos|pegable|13"]
        );
        limpiar(dir);
    }

    #[test]
    fn los_pedidos_salen_en_orden() {
        let (carga, dir) = cargar(
            "orden",
            r#"
            flint.register_command("x", "X", function()
              flint.insert_text("a")
              flint.replace_selection("b")
              flint.set_clipboard("c")
              flint.set_cursor(7, 3)
              flint.action("save")
              flint.status("listo")
            end)
            "#,
        );
        let efectos = carga
            .bridge
            .invoke(&carga.commands[0], ctx_de_prueba())
            .expect("el script corre");
        assert_eq!(
            textos(&efectos),
            vec![
                "insert:a",
                "replace:b",
                "clipboard:c",
                "cursor:7:3",
                "action:save",
                "status:listo",
            ]
        );
        limpiar(dir);
    }

    #[test]
    fn un_script_que_falla_no_deja_nada_a_medias() {
        // Lo importante: el `insert_text` de antes del error NO se aplica.
        // Media intención aplicada es peor que ninguna.
        let (carga, dir) = cargar(
            "falla",
            r#"
            flint.register_command("x", "X", function()
              flint.insert_text("no debería verse")
              error("me caí")
            end)
            "#,
        );
        let r = carga.bridge.invoke(&carga.commands[0], ctx_de_prueba());
        assert!(r.is_err(), "tenía que fallar");
        // Y la próxima llamada arranca limpia, sin arrastrar lo anterior.
        let (carga2, dir2) = cargar(
            "falla2",
            r#"flint.register_command("y", "Y", function() flint.status("ok") end)"#,
        );
        let efectos = carga2
            .bridge
            .invoke(&carga2.commands[0], ctx_de_prueba())
            .expect("el script corre");
        assert_eq!(textos(&efectos), vec!["status:ok"]);
        limpiar(dir);
        limpiar(dir2);
    }

    #[test]
    fn las_teclas_se_juntan_para_resolverlas_despues() {
        let (carga, dir) = cargar(
            "teclas",
            r#"
            flint.bind("f5", "mio")
            flint.bind("ctrl+j", "goto_line")
            flint.bind("g", "mio", true)
            flint.register_command("mio", "Mío", function() end)
            "#,
        );
        assert!(carga.errors.is_empty(), "{:?}", carga.errors);
        let pares: Vec<(&str, &str, bool)> = carga
            .binds
            .iter()
            .map(|b| (b.tecla.as_str(), b.destino.as_str(), b.modal))
            .collect();
        assert_eq!(
            pares,
            vec![
                ("f5", "mio", false),
                ("ctrl+j", "goto_line", false),
                ("g", "mio", true),
            ]
        );
        limpiar(dir);
    }

    #[test]
    fn un_script_roto_no_voltea_a_los_demas() {
        let dir = std::env::temp_dir().join(format!("flint-plugins-{}-roto", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a_roto.lua"), "esto no es lua válido ((").unwrap();
        std::fs::write(
            dir.join("b_sano.lua"),
            r#"flint.register_command("ok", "OK", function() end)"#,
        )
        .unwrap();
        let carga = PluginBridge::load(std::slice::from_ref(&dir));
        assert_eq!(carga.commands.len(), 1, "el script sano tenía que cargar");
        assert_eq!(carga.errors.len(), 1, "y el roto tenía que avisar");
        limpiar(dir);
    }

    #[test]
    fn el_ejemplo_que_se_distribuye_carga_sin_errores() {
        // El `ejemplo.lua` que va adentro del .deb es también la
        // documentación de la API: si deja de cargar, lo primero que ve
        // alguien que instala Flint es un error.
        let carga = PluginBridge::load(&[PathBuf::from("plugins")]);
        assert!(carga.errors.is_empty(), "{:?}", carga.errors);
        assert!(
            carga.commands.len() >= 3,
            "el ejemplo registra varios comandos"
        );
        assert!(!carga.binds.is_empty(), "el ejemplo ata teclas");
    }
}
