use std::cell::RefCell;
use std::path::Path;
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

/// El puente entre Flint y Lua. La API que ve un script es deliberadamente
/// chica: `flint.register_command(id, etiqueta, función)` para sumar un
/// comando a la paleta, y dentro de esa función, `flint.status(texto)` y
/// `flint.insert_text(texto)` para hacer algo real cuando el usuario lo
/// ejecuta, más `flint.filename()` / `flint.line_count()` de solo lectura.
/// Nada de esto puede tocar el buffer más allá de insertar texto en el
/// cursor — ni asignar atajos, ni leer o mover la selección, ni abrir
/// archivos. Es un primer corte real pero angosto a propósito.
pub struct PluginBridge {
    lua: Lua,
    pending_status: Rc<RefCell<Option<String>>>,
    pending_inserts: Rc<RefCell<Vec<String>>>,
    ctx_filename: Rc<RefCell<String>>,
    ctx_line_count: Rc<RefCell<usize>>,
}

impl PluginBridge {
    /// Carga todos los `*.lua` de `dir` (si existe). Devuelve el puente, la
    /// lista de comandos que los scripts registraron, y cualquier error de
    /// carga (un script con errores no aborta a los demás).
    pub fn load(dir: &Path) -> (PluginBridge, Vec<PluginCommand>, Vec<String>) {
        let lua = Lua::new();
        let pending_status = Rc::new(RefCell::new(None));
        let pending_inserts = Rc::new(RefCell::new(Vec::new()));
        let ctx_filename = Rc::new(RefCell::new(String::new()));
        let ctx_line_count = Rc::new(RefCell::new(0usize));
        let registered: Rc<RefCell<Vec<(String, String, RegistryKey)>>> =
            Rc::new(RefCell::new(Vec::new()));

        let mut errors = Vec::new();
        if let Err(e) = install_api(
            &lua,
            &registered,
            &pending_status,
            &pending_inserts,
            &ctx_filename,
            &ctx_line_count,
        ) {
            errors.push(format!("No se pudo preparar la API de plugins: {e}"));
        }

        if dir.is_dir() {
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
                    paths.sort();
                    for path in paths {
                        if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                            continue;
                        }
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
        let commands = std::mem::take(&mut *registered.borrow_mut())
            .into_iter()
            .map(|(id, label, key)| PluginCommand { id, label, key })
            .collect();

        (
            PluginBridge {
                lua,
                pending_status,
                pending_inserts,
                ctx_filename,
                ctx_line_count,
            },
            commands,
            errors,
        )
    }

    /// Ejecuta el comando de un plugin. `filename`/`line_count` son lo que
    /// `flint.filename()`/`flint.line_count()` verán dentro de la llamada.
    /// Devuelve el texto de estado que pidió mostrar (si pidió alguno) y la
    /// lista de textos a insertar en el cursor, en orden.
    pub fn invoke(
        &self,
        cmd: &PluginCommand,
        filename: &str,
        line_count: usize,
    ) -> Result<(Option<String>, Vec<String>), String> {
        *self.ctx_filename.borrow_mut() = filename.to_string();
        *self.ctx_line_count.borrow_mut() = line_count;
        self.pending_status.borrow_mut().take();
        self.pending_inserts.borrow_mut().clear();

        let func: Function = self
            .lua
            .registry_value(&cmd.key)
            .map_err(|e| e.to_string())?;
        func.call::<()>(()).map_err(|e| e.to_string())?;

        Ok((
            self.pending_status.borrow_mut().take(),
            self.pending_inserts.borrow_mut().drain(..).collect(),
        ))
    }
}

#[allow(clippy::type_complexity)]
fn install_api(
    lua: &Lua,
    registered: &Rc<RefCell<Vec<(String, String, RegistryKey)>>>,
    pending_status: &Rc<RefCell<Option<String>>>,
    pending_inserts: &Rc<RefCell<Vec<String>>>,
    ctx_filename: &Rc<RefCell<String>>,
    ctx_line_count: &Rc<RefCell<usize>>,
) -> mlua::Result<()> {
    let flint = lua.create_table()?;

    let reg = registered.clone();
    let register_command =
        lua.create_function(move |lua_ctx, (id, label, f): (String, String, Function)| {
            let key = lua_ctx.create_registry_value(f)?;
            reg.borrow_mut().push((id, label, key));
            Ok(())
        })?;
    flint.set("register_command", register_command)?;

    let status_cell = pending_status.clone();
    let status_fn = lua.create_function(move |_, text: String| {
        *status_cell.borrow_mut() = Some(text);
        Ok(())
    })?;
    flint.set("status", status_fn)?;

    let insert_cell = pending_inserts.clone();
    let insert_fn = lua.create_function(move |_, text: String| {
        insert_cell.borrow_mut().push(text);
        Ok(())
    })?;
    flint.set("insert_text", insert_fn)?;

    let filename_cell = ctx_filename.clone();
    let filename_fn = lua.create_function(move |_, ()| Ok(filename_cell.borrow().clone()))?;
    flint.set("filename", filename_fn)?;

    let line_count_cell = ctx_line_count.clone();
    let line_count_fn = lua.create_function(move |_, ()| Ok(*line_count_cell.borrow()))?;
    flint.set("line_count", line_count_fn)?;

    lua.globals().set("flint", flint)?;
    Ok(())
}
