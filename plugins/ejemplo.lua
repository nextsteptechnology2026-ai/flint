-- Plugin de ejemplo para Flint.
--
-- Un script ve una sola tabla global, `flint`. Al cargarse puede registrar
-- comandos (aparecen en la paleta, Ctrl+P) y atar teclas; adentro de un
-- comando puede leer el estado del editor y pedir cambios.
--
-- Todo lo que cambia el buffer se pide, no se hace: Flint aplica los pedidos
-- cuando el script terminó. Si el script falla en el medio no se aplica
-- ninguno, así que un plugin roto no deja el editor a mitad de camino.
--
-- Lectura (adentro de un comando o de un evento):
--   flint.filename()    ruta del archivo, "" si no tiene
--   flint.language()    id del lenguaje ("rust", "python"…), nil si no hay
--   flint.line_count()  cuántas líneas tiene
--   flint.cursor()      línea y columna del cursor, contando desde 1
--   flint.line(n)       el texto de la línea n, desde 1
--   flint.text()        todo el contenido
--   flint.selection()   lo seleccionado, "" si no hay
--   flint.clipboard()   lo que hay para pegar
--
-- Pedidos (adentro de un comando o de un evento):
--   flint.status(texto)             mostrarlo en la barra de estado
--   flint.insert_text(texto)        insertarlo en cada cursor
--   flint.replace_selection(texto)  reemplazar lo seleccionado
--   flint.set_clipboard(texto)      dejarlo en el portapapeles
--   flint.set_cursor(linea, col)    mover el cursor, desde 1
--   flint.action(nombre)            ejecutar una acción de Flint por nombre
--                                   (la lista sale de `flint --actions`)
--
-- Registro (al cargarse):
--   flint.register_command(id, etiqueta, funcion)
--   flint.bind(tecla, destino [, modal])
--     `destino` es el id de un comando propio o el nombre de una acción.
--     `modal` en true ata la tecla en la capa modal NORMAL en vez de la
--     directa. La tecla se escribe como en config.toml: "ctrl+j", "f5", "g".
--     Las teclas de un plugin se miran antes que las de fábrica y las de
--     config.toml, así que conviene elegir teclas libres.
--   flint.on(evento, funcion)
--     Correr `funcion` cuando pasa algo: "open" (se abrió un archivo),
--     "before_save" (se va a guardar; lo que pida se aplica antes de
--     escribir) o "save" (se guardó).
--   flint.define_language{ id = ..., extensions = {...}, grammar = "x.so",
--                          highlights = "highlights.scm", ... }
--     Traer un lenguaje nuevo con su gramática de tree-sitter compilada.
--     Todas las claves están en el MANUAL.md ("Lenguajes propios"), y
--     tests/gramatica-prueba/ tiene una gramática mínima de muestra.

flint.register_command("fecha", "Insertar fecha de hoy", function()
    flint.insert_text(os.date("%Y-%m-%d"))
    flint.status("Fecha insertada")
end)

flint.register_command("info-archivo", "Info del archivo actual", function()
    local nombre = flint.filename()
    if nombre == "" then
        nombre = "[sin nombre]"
    end
    local linea, col = flint.cursor()
    flint.status(string.format(
        "%s · %s · %d línea(s) · cursor en %d:%d",
        nombre,
        flint.language() or "texto plano",
        flint.line_count(),
        linea,
        col
    ))
end)

-- Lee la selección y la devuelve cambiada: el caso que antes no se podía.
flint.register_command("mayusculas", "Selección a MAYÚSCULAS", function()
    local sel = flint.selection()
    if sel == "" then
        flint.status("No hay nada seleccionado")
        return
    end
    flint.replace_selection(sel:upper())
    flint.status("Pasado a mayúsculas")
end)

-- Deja el archivo entero en el portapapeles, sin tocar la selección.
flint.register_command("copiar-archivo", "Copiar el archivo entero", function()
    flint.set_clipboard(flint.text())
    flint.status("El archivo entero quedó en el portapapeles")
end)

-- Un comando que se apoya en las acciones de Flint en vez de reimplementarlas.
flint.register_command("guardar-y-avisar", "Guardar avisando el tamaño", function()
    local lineas = flint.line_count()
    flint.action("save")
    flint.status("Guardado, " .. lineas .. " línea(s)")
end)

-- Un evento: avisar al guardar si quedaron espacios al final de alguna
-- línea. Viene apagado porque este archivo lo carga cualquiera que instale
-- el .deb, y el aviso reemplaza el mensaje de "Guardado". Para probarlo,
-- poner `true` en una copia en ~/.config/flint/plugins/.
local AVISAR_ESPACIOS = false

if AVISAR_ESPACIOS then
    flint.on("save", function()
        local con_espacios = 0
        for n = 1, flint.line_count() do
            if flint.line(n):find("[ \t]+$") then
                con_espacios = con_espacios + 1
            end
        end
        if con_espacios > 0 then
            flint.status("Guardado · " .. con_espacios .. " línea(s) con espacios al final")
        end
    end)
end

-- Atajos propios. El primero apunta a un comando de este mismo script; el
-- segundo, a una acción de Flint, y el tercero vive en la capa modal NORMAL.
-- Las tres teclas están libres de fábrica en los tres perfiles.
flint.bind("f5", "info-archivo")
flint.bind("ctrl+j", "goto_line")
flint.bind("t", "fecha", true)
