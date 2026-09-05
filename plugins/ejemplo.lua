-- Plugin de ejemplo para Flint.
-- La API disponible hoy es chica a propósito: registrar comandos que
-- aparecen en la paleta (Ctrl+P), y desde ellos, mostrar un mensaje de
-- estado y/o insertar texto en el cursor. Nada más todavía — ver el README
-- para la lista completa de lo que falta (atajos propios, leer/mover la
-- selección, abrir archivos...).

flint.register_command("fecha", "Insertar fecha de hoy", function()
    flint.insert_text(os.date("%Y-%m-%d"))
    flint.status("Fecha insertada")
end)

flint.register_command("info-archivo", "Info del archivo actual", function()
    local nombre = flint.filename()
    if nombre == "" then
        nombre = "[sin nombre]"
    end
    flint.status(nombre .. " — " .. flint.line_count() .. " línea(s)")
end)

flint.register_command("saludo", "Insertar saludo desde Lua", function()
    flint.insert_text("¡Hola desde un plugin de Lua!")
end)
