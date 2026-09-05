# Flint

[![CI](https://github.com/nextsteptechnology2026-ai/flint/actions/workflows/ci.yml/badge.svg)](https://github.com/nextsteptechnology2026-ai/flint/actions/workflows/ci.yml)
[![Licencia: MIT](https://img.shields.io/badge/licencia-MIT-blue.svg)](LICENSE)

Editor de terminal en Rust. Abre como `nano` —las teclas hacen lo que dicen— y suma, encima, una capa modal opcional a un `F2` de distancia. Resaltado de sintaxis y LSP reales vía tree-sitter, multi-cursor, temas en TOML con recarga en caliente y plugins en Lua.

```
 Flint   demo.rs
   1 use std::collections::HashMap;
   2
   3 /// Cuenta cuántas veces aparece cada palabra.
   4 fn contar(texto: &str) -> HashMap<&str, usize> {
   5     let mut conteo = HashMap::new();
   6     for palabra in texto.split_whitespace() {
   7         *conteo.entry(palabra).or_insert(0) += 1;
   8     }
   9     conteo
  10 }
  11
  12 fn main() {
  13     let conteo = contar("uno dos dos tres tres tres");
  14     println!("{conteo:?}");
  15 }
  16
     ~
 Iniciando rust-analyzer… · rust-analyzer listo (sync incremental)
 ^S Guardar  ^Q Salir  ^F Buscar  ^R Reemplazar  ^Z Deshacer  ^Y Rehacer
 F2 Capa modal  ^P Paleta  ^Espacio Autocompletar  ^G Diagnóstico  ^D +cur
```

## Por qué

Los editores de terminal suelen obligarte a elegir de entrada: la simplicidad de `nano`, y te quedás sin herramientas; o el modelo modal de Vim/Helix, y pagás la curva antes de escribir la primera letra.

Flint no te hace elegir. Arranca en modo directo, sin nada que aprender, y todo lo demás —capa modal, multi-cursor, LSP, plugins— está ahí cuando lo quieras, no antes.

## Instalación

**Paquete `.deb`** (Debian, Ubuntu, Kali y derivados). La máquina donde se instala **no** necesita tener Rust:

```sh
bash packaging/build-deb.sh                       # compila y arma el paquete
sudo apt install ./target/deb/flint_*_amd64.deb
```

**Desde el código** (necesita Rust ≥ 1.85, por la edición 2024):

```sh
cargo install --path .        # instala en ~/.cargo/bin
cargo run -- archivo.txt      # o simplemente probarlo sin instalar
```

## Uso

```sh
flint archivo.txt                          # abre (o crea) un archivo
flint                                      # buffer sin nombre
flint --profile vim archivo.rs             # perfil de teclado Vim (o emacs, flint)
flint --theme theme.example.toml notas.md  # con un tema propio
```

Lo mínimo para moverse; todo lo demás está en **[MANUAL.md](MANUAL.md)**:

| Tecla | Qué hace |
|---|---|
| `Ctrl+S` / `Ctrl+Q` | Guardar / salir |
| `Ctrl+F` / `Ctrl+R` | Buscar / reemplazar |
| `Ctrl+Z` / `Ctrl+Y` | Deshacer / rehacer |
| `Ctrl+P` | Paleta de comandos (todo lo que Flint sabe hacer, con búsqueda difusa) |
| `Ctrl+D` | Sumar un cursor en la siguiente aparición de lo seleccionado |
| `Tab` / `Shift+Tab` | Indentar / des-indentar el bloque seleccionado |
| `F2` | Prender o apagar la capa modal |

¿Perdido? `Ctrl+P` lista todos los comandos por nombre, y la barra de ayuda de abajo cambia según la capa activa.

## Qué trae

- **Edición sin sorpresas**: buffer en rope (`ropey`), selección con teclado y mouse, deshacer/rehacer agrupado por ráfagas, portapapeles **del sistema** (no un registro interno), múltiples buffers con pestañas, CRLF respetado, búsqueda y reemplazo con o sin regex, ajuste de línea opcional.
- **Capa modal opcional** (`F2`): modelo selección→acción estilo Kakoune/Helix, con **selección estructural** sobre el árbol de tree-sitter (`n` expande al nodo que contiene la selección, y otra vez sube al padre).
- **Multi-cursor de verdad**: `Ctrl+D` y `Alt+clic`; escribir, borrar y deshacer actúan sobre todos los cursores a la vez, de forma atómica.
- **Resaltado y LSP reales**: tree-sitter para Rust, Python, JSON y TOML; cliente LSP propio (JSON-RPC sobre stdio, con sync incremental) conectado hoy a `rust-analyzer` — diagnósticos subrayados en el rango exacto y autocompletado que filtra por lo que ya escribiste. Si falta el servidor, Flint ofrece instalarlo; nunca lo hace en silencio.
- **Autocompletado en cualquier archivo**: con servidor LSP, sus sugerencias; sin él, las palabras que ya escribiste en el archivo (como `Ctrl+N` en Vim), sin distinguir mayúsculas y ordenadas por cercanía al cursor.
- **Tuyo**: temas en `~/.config/flint/theme.toml` que se recargan solos al guardarlos, perfiles de teclado Flint/Vim/Emacs sobre una tabla tecla→acción, y plugins en Lua que registran comandos en la paleta (se cargan desde `./plugins`, `~/.config/flint/plugins/` y `/usr/share/flint/plugins/`).
- **Anchos de pantalla reales**: tabuladores, CJK, emoji y acentos combinantes se miden en columnas de terminal, no en caracteres — el cursor cae donde está el texto.

## Estado

Prototipo funcional y en uso, no un 1.0. Lo que falta —y por qué— está en **[pendiente.txt](pendiente.txt)**, verificado contra el código: LSP solo para Rust, perfiles de teclado que todavía no se definen desde un archivo, API de plugins angosta, y tests que hoy cubren la aritmética de columnas y la indentación pero no la edición completa.

## Licencia

MIT — © 2026 Next Step Technology SpA. Ver [LICENSE](LICENSE).

---

# Cómo se construyó, y qué se probó

De acá para abajo está el registro del desarrollo, fase por fase: qué se implementó, qué bugs reales aparecieron al probarlo y cómo se arreglaron, y qué se dejó afuera a propósito. Es la parte interesante si te importa el *cómo*, y no hace falta leerla para usar Flint.

Una advertencia de lectura: algunas limitaciones que se mencionan en las fases tempranas se resolvieron después. La sección **"Ya resuelto"** del final es la que manda.

## Qué hace (Fase 0)

- **Buffer en rope** (`ropey`) — edición no modal desde que abre, sin curva de aprendizaje.
- **Teclado**: flechas, Home/End, Página arriba/abajo, escribir, `Enter`, `Backspace`, `Delete`.
- **Selección**: `Shift` + flechas, o click-y-arrastrar con el mouse. Escribir sobre una selección la reemplaza, como en cualquier editor moderno.
- **Mouse**: click posiciona el cursor, arrastrar selecciona, la rueda desplaza la vista.
- **Buscar** (`Ctrl+F`): busca hacia adelante desde el cursor, da la vuelta al llegar al final. Enter con el campo vacío repite la última búsqueda.
- **Reemplazar** (`Ctrl+R`): pide el texto a buscar y el reemplazo, y sustituye todas las coincidencias.
- **Guardar** (`Ctrl+S`): si el buffer no tiene nombre todavía, pide uno (Guardar como).
- **Deshacer** (`Ctrl+Z`) / **Rehacer** (`Ctrl+Y`): las ediciones consecutivas del mismo tipo (escribir seguido, borrar seguido) hechas en menos de 700ms se agrupan en un solo paso, así deshacer no va letra por letra.
- **Salir** (`Ctrl+Q`): si hay cambios sin guardar, pregunta (s = guardar y salir, n = salir sin guardar, Esc = cancelar) — igual que nano.
- Barra de título (archivo + indicador de modificado) y barra de ayuda con los atajos, siempre visibles.

## Qué hace (Fase 1)

- **Resaltado de sintaxis real** vía [tree-sitter](https://tree-sitter.github.io/), no aproximaciones por regex. Cubre **Rust, Python, JSON y TOML** por extensión de archivo (`.rs .py .json .toml`); cualquier otra extensión se edita en texto plano, sin romperse. Se recalcula solo cuando el contenido cambia, no en cada redibujado.
- **Cliente LSP real** (JSON-RPC sobre stdio, en un hilo aparte para no bloquear la interfaz). Hoy solo **Rust está conectado a un servidor** (`rust-analyzer`); es el único lenguaje de los cuatro para el que Flint sabe qué servidor lanzar.
  - **Diagnósticos**: se muestran como marca en el margen (✖ error, ▲ warning), resumen en la barra de título, y `Ctrl+G` salta al siguiente y muestra su mensaje real del servidor.
  - **Autocompletado** (`Ctrl+Espacio`): pide sugerencias al servidor en la posición del cursor y las muestra en una lista flotante (↑↓ para elegir, `Enter`/`Tab` para insertar, `Esc` para cancelar). Inserta el texto de la sugerencia tal cual — no reemplaza un prefijo ya escrito ni aplica ediciones estructuradas (`TextEdit`) más complejas.
  - **Sincronización**: se manda el documento completo (no incremental) medio segundo después de la última tecla, para no saturar al servidor mientras se escribe.
  - **Instalación guiada**: si el servidor de un lenguaje conectado no está en el `PATH`, Flint pregunta antes de instalarlo — nunca en silencio, nunca hace falta salir a la terminal a mano. Hoy sabe instalar `rust-analyzer` vía `rustup component add rust-analyzer`.

Todo esto se probó a mano en sesiones de tmux, incluyendo contra un `rust-analyzer` real: resaltado en los cuatro lenguajes, diagnósticos reales (`mismatched types`, `cannot find value ...`) sobre un proyecto Cargo de prueba, salto entre diagnósticos, autocompletado real (`std::` mostrando `alloc::`, `any::`, `arch::`…) insertado desde la lista, y el diálogo de instalación guiada tanto aceptando como rechazando.

## Qué hace (Fase 2)

`F2` activa y desactiva una **capa modal opcional**, encima del modo directo — el editor sigue arrancando en modo directo, tal como en Fase 0/1; la capa modal es algo que se pide, no el punto de partida.

- **NORMAL** (selección→acción, al estilo Kakoune/Helix — al revés que el "verbo antes que sustantivo" de Vim): las flechas/`hjkl` mueven y colapsan la selección; con `Shift` la extienden. `w` selecciona la palabra siguiente, `x` selecciona la línea actual (repetida, extiende una línea más — igual que en Kakoune). Sobre lo que quede seleccionado (por teclado, por `Shift`, o arrastrado con el mouse — las tres formas alimentan la misma selección): `d` borra, `c` cambia (borra y entra a INSERT), `y` copia a un registro interno, `p` pega. `i`/`a`/`I`/`A` entran a INSERT en distintos puntos: inicio/fin de la selección, o inicio/fin de línea. `o`/`O` abren una línea nueva abajo/arriba y entran a INSERT. `u`/`U` deshacen/rehacen sin necesidad de `Ctrl`.
- **INSERT**: exactamente el tecleo literal del modo directo. `Esc` vuelve a NORMAL (no a modo directo — `F2` es lo que apaga la capa modal entera).
- **Selección estructural** (`n`, solo en NORMAL): expande la selección al nodo del árbol de tree-sitter que la contiene — y si ya coincide con un nodo exacto, sube a su nodo padre, así presionarla varias veces va ampliando la selección un nivel del árbol por vez (`identifier` → `binary_expression` → `block` → …). Requiere que el archivo tenga resaltado tree-sitter activo (Rust, Python, JSON, TOML); en cualquier otro tipo de archivo avisa que no hay árbol de sintaxis disponible.
- Los atajos con `Ctrl` (guardar, buscar, reemplazar, deshacer/rehacer, diagnósticos, autocompletar) funcionan igual en las tres capas — nunca quedan atrapados detrás del modo modal.
- La barra de título muestra `[NORMAL]` / `[INSERT]` (nada en modo directo), y la barra de ayuda cambia su contenido según la capa activa, para que los atajos disponibles sigan siendo descubribles sin memorizarlos.

Se probó a mano: seleccionar palabra/línea y borrarlas, deshacer y rehacer dentro de la capa modal, copiar y pegar entre líneas, abrir línea arriba/abajo, entrar a INSERT por las cuatro variantes, seleccionar con el mouse (arrastrando) y borrar esa selección con `d` desde NORMAL, expandir la selección estructural tres niveles seguidos sobre un archivo Rust real, y el ida-y-vuelta completo `F2` → NORMAL → INSERT → NORMAL → `F2` → modo directo con tecleo normal funcionando después.

## Multi-cursor

Lo que en la primera vuelta de la Fase 2 había quedado fuera a propósito: selecciones múltiples de verdad, no solo una. El editor pasó de tener un cursor primario a una lista de selecciones (una primaria + cualquier cantidad de secundarias), y **toda** operación de edición y movimiento se reescribió para actuar sobre esa lista completa a la vez, no solo sobre la primaria.

- **Armar el multi-cursor**: `Ctrl+D` agrega una selección nueva sobre la siguiente aparición del texto ya seleccionado (como Ctrl+D en VS Code/Sublime) — selecciona una palabra y repetí `Ctrl+D` para ir sumando cursores en cada ocurrencia siguiente, saltando las que ya están y dando la vuelta al llegar al final. `Alt+clic` agrega un cursor suelto donde se hace clic, sin tocar los que ya había; un clic normal (sin Alt) vuelve a un solo cursor. `Esc` descarta los cursores extra.
- **Escribir, borrar, deshacer, todo a la vez**: tipear, `Enter`, `Backspace`, `Delete`, y las acciones de la capa modal (`d`/`c`/`y`/`p`/`i`/`a`/`I`/`A`/`o`/`O`) se aplican a cada cursor de forma simultánea y atómica — un solo `Ctrl+Z` deshace la edición completa en todos los cursores a la vez, sin importar si están en la misma línea o en líneas distintas. `n` (selección estructural) expande cada cursor por su propia rama del árbol de tree-sitter, de forma independiente.
- **Se ve**: la barra de título muestra `×N cursores` cuando hay más de uno; cada selección con rango se resalta en ámbar (igual que la primaria) y cada cursor secundario sin rango se marca con un carácter en teal, porque la terminal solo puede mostrar un cursor real parpadeando — el de la primaria.

Se probó a mano, incluyendo el caso que de verdad hace falta para confiar en esto: seleccionar una palabra repetida tres veces con `Ctrl+D`, escribir sobre las tres a la vez, deshacer esa edición completa en un solo `Ctrl+Z`; `Alt+clic` para poner cuatro cursores en cuatro líneas distintas y escribir/`Enter`/`Backspace` en las cuatro a la vez (el caso que de verdad ejercita que los saltos de línea de un cursor no corrompan la posición de los demás); y `d`/`y`/`p` con selecciones múltiples con rango.

**Un bug real que apareció al probar, y cómo se arregló**: la primera versión procesaba las ediciones de derecha a izquierda y calculaba la posición final de cada cursor *mientras* iba editando — pero una edición más a la izquierda corre las posiciones de todo lo que quedó a su derecha, así que una selección ya procesada terminaba con una posición vieja e inválida (el editor directamente crasheaba con un índice fuera de rango al escribir sobre tres selecciones en la misma línea). La versión final separa las dos cosas: primero calcula la posición final de cada cursor en una pasada aparte (de izquierda a derecha, acumulando cuánto se corrió el texto), y recién después aplica las mutaciones reales sobre el rope (esas sí, de derecha a izquierda). Quedó cubierto por las pruebas de arriba.

**Lo que se dejó fuera a propósito**: el registro de copiar/pegar es uno solo compartido (si copiás de varias selecciones a la vez, se guardan unidas con salto de línea, y pegar mete ese mismo bloque en cada cursor) — no un registro por selección como en Kakoune de verdad. No hay forma de arrastrar el mouse para extender un cursor secundario ya puesto (`Alt+clic` solo pone un punto). Selección de palabra sigue mirando nada más que la línea actual, igual que en modo un-solo-cursor.

## Qué hace (Fase 3)

Extensibilidad y perfiles: personalizar Flint ya no requiere tocar el código, aunque el mecanismo de fondo (una tabla tecla→acción) es el mismo tanto para un perfil incluido como para un remapeo propio.

- **Paleta de comandos** (`Ctrl+P`): lista todo lo que Flint sabe hacer — guardar, buscar, alternar la capa modal, cambiar de perfil de teclado, cualquier comando que un plugin haya registrado — con filtro difuso mientras se escribe (no hace falta el nombre exacto ni el orden exacto de las letras) y `↑↓`/`Enter` para elegir y ejecutar.
- **Perfiles de teclado remapeables**: por debajo de los atajos hay una sola tabla de datos (tecla → `Action`), no un montón de `match` sueltos — así que un perfil nuevo es solo otra tabla. Vienen tres:
  - **Flint** (por defecto): los atajos ya descritos arriba.
  - **Vim** (sabor): `u`/`Ctrl+R` deshacen/rehacen como en Vim de verdad, y `x` borra el carácter bajo el cursor en vez de seleccionar la línea. No es una emulación completa — no hay secuencias `dd`/`ciw`, porque el modelo de Flint sigue siendo selección→acción, no verbo+movimiento — pero lo que sí se puede mapear a una sola tecla, se mapeó de verdad.
  - **Emacs** (sabor): `Ctrl+X Ctrl+S` guarda y `Ctrl+X Ctrl+C` sale — el prefijo de dos teclas más reconocible de Emacs, con un mecanismo genérico de teclas-prefijo por si hace falta para algo más—, `Ctrl+S` busca y `Ctrl+G` cancela, igual que en Emacs real.
  - Se elige con `--profile vim|emacs|flint` al arrancar, o cambiando en caliente desde la paleta de comandos (búsqueda "perfil").
- **Plugins en Lua**: Flint carga cualquier `plugins/*.lua` (relativo al directorio desde donde se arranca) al iniciar. La API de hoy es angosta a propósito: `flint.register_command(id, etiqueta, función)` agrega un comando a la paleta; dentro de esa función, `flint.status(texto)` muestra un mensaje, `flint.insert_text(texto)` inserta en el cursor, y `flint.filename()`/`flint.line_count()` son de solo lectura. Ver `plugins/ejemplo.lua` — tres comandos reales: insertar la fecha de hoy, mostrar info del archivo, insertar un saludo.

Se probó a mano: los tres comandos del plugin de ejemplo ejecutándose de verdad (fecha real del sistema insertada, nombre de archivo y cantidad de líneas leídos correctamente); cambio de perfil en caliente desde la paleta a Vim y a Emacs; `u`/`Ctrl+R` y `x` con el perfil Vim; la secuencia `Ctrl+X Ctrl+S` completa (incluyendo el estado intermedio "…" mientras espera la segunda tecla) y su cancelación con una tecla no válida, con el perfil Emacs; `--profile emacs` y `--profile klingon` (perfil inexistente, cae a Flint con aviso) desde la línea de comandos.

**Un bug real que apareció al probar, y cómo se arregló**: la primera versión de la carga de plugins usaba `Rc::try_unwrap` para sacar la lista de comandos registrados fuera de un `Rc<RefCell<...>>` compartido — pero la propia función de Lua que `register_command` guarda en el registro de Lua conserva su propio clon de ese `Rc` mientras el intérprete siga vivo (que es exactamente lo que se necesita, para poder invocar esos comandos más tarde). Eso significa que el conteo de referencias nunca baja a 1, así que `try_unwrap` fallaba en silencio y devolvía una lista vacía — los plugins parecían cargar pero la paleta no mostraba ningún comando. Se arregló vaciando el `Vec` de adentro del `RefCell` en vez de intentar tomar posesión del `Rc` entero.

**Otro más chico, encontrado en la misma pasada**: la paleta de comandos se cerraba sola al presionar `Enter` sin ninguna coincidencia, en vez de quedarse abierta para seguir corrigiendo la búsqueda. Los dos quedaron cubiertos por las pruebas de arriba.

**Lo que se dejó fuera a propósito**: los plugins de Lua no pueden todavía asignar sus propios atajos de teclado, ni leer o mover la selección, ni tocar el buffer más allá de insertar texto en el cursor — es un primer corte real pero angosto, para probar el mecanismo sin abrir una superficie enorme de una vez. F2 (la capa modal) sigue fijo y no es parte del sistema de acciones remapeable. Los perfiles Vim y Emacs son un sabor sobre el mismo modelo de Flint, no una emulación — no hay `:` de Vim ni el resto de los atajos de Emacs.

## Qué hace (temas)

Personalización visual real, sin tocar código ni recompilar: un `theme.toml` opcional con colores, y dos toques de "forma" que sí tienen sentido en una terminal (transparencia de las barras y forma del cursor).

- **Dónde va**: `~/.config/flint/theme.toml` se carga solo si existe; `--theme <ruta>` (por ejemplo `--theme theme.example.toml`) manda por encima de esa ubicación por defecto. Sin ninguno de los dos, Flint usa la paleta ámbar con la que arrancó — nada cambia si no tocás nada.
- **Qué se puede tocar**: `[colors]` — los colores de las barras, la selección, los cursores secundarios, los popups y los tres niveles de diagnóstico; `[syntax]` — cada categoría de resaltado (`keyword`, `function`, `type`, `string`, `comment`, `number`, `constant`, `operator`, `punctuation`, `property`, `attribute`); `[appearance]` — `transparent_bars` (la barra de título/ayuda/mensaje dejan de pintar su propio fondo y heredan el de tu terminal — el área de texto en sí *siempre* se comportó así, nunca pintó su propio fondo, así que si tu terminal tiene transparencia/blur configurados ya se veía ahí de por sí) , `cursor_style` (`block`, `bar` o `underline`, con `cursor_blink`), aplicado de verdad vía las secuencias de cursor de la terminal (`crossterm::cursor::SetCursorStyle`), y devuelto a la forma por defecto de tu terminal al salir, y `tab_width` (1 a 16, por defecto 4) — cuántas columnas ocupa un tabulador al dibujarse, sin tocar el archivo: el `\t` sigue siendo un solo carácter.
- **Ningún color es obligatorio**: cada entrada que falta, o que no se puede interpretar (`"no-es-un-color"` en vez de `"#rrggbb"`), se queda en su valor por defecto — un archivo con un solo color definido es un archivo válido, y un color mal escrito no rompe la carga del resto.
- Ver `theme.example.toml` — una paleta azul/violeta completa, bien distinta de la de fábrica, para probar que el mecanismo funciona de un vistazo.

Se probó a mano: el tema de ejemplo cambiando de verdad los colores de barras, texto y sintaxis; un color inválido cayendo a su default con el aviso correcto en la barra de estado, sin romper el resto del archivo ni la app; `transparent_bars = true` quitando de verdad el código de fondo de las barras (confirmado mirando la salida ANSI cruda, no solo believing la captura de pantalla); y que sin ningún `theme.toml` presente, todo se ve exactamente igual que antes de que este archivo existiera.

## Qué hace (portapapeles del sistema)

Copiar/cortar/pegar habla de verdad con el portapapeles del sistema operativo (vía [`arboard`](https://crates.io/crates/arboard)), no solo con un registro interno de Flint — lo que se copia en Flint se puede pegar en cualquier otra aplicación, y viceversa.

- **Modo directo**: `Ctrl+C` copia la selección, `Ctrl+X` la corta, `Ctrl+V` pega — las mismas teclas que en prácticamente cualquier otro programa. (En el perfil Emacs, `Ctrl+X` sigue siendo el prefijo de dos teclas — `Ctrl+X Ctrl+S`/`Ctrl+X Ctrl+C` —, así que ahí `Ctrl+X` solo no corta.)
- **Capa modal**: `y`/`d`/`c`/`p` ahora leen y escriben el mismo portapapeles del sistema, no un registro aparte.
- **Si el portapapeles del sistema no está disponible** (sin servidor gráfico, por ejemplo) Flint no se cae: copiar/cortar/pegar siguen funcionando con un registro interno, en memoria, exactamente como antes de este cambio — la única diferencia es que ese texto ya no sale hacia otras aplicaciones.
- Pegar prioriza el portapapeles del sistema sobre el registro interno — así, si copiaste algo en *otra* app después de tu última copia en Flint, `p`/`Ctrl+V` trae lo más reciente, no algo viejo.

Se probó de punta a punta contra el portapapeles real, no simulado: seteé texto desde **afuera** del proceso con `xclip` y lo pegué dentro de Flint con `Ctrl+V`; copié texto **dentro** de Flint con `Ctrl+C` y lo leí desde afuera con `xclip -o`; lo mismo con `Ctrl+X` (cortar) y con `y`/`p` de la capa modal — las cuatro direcciones confirmadas, no solo una.

## Qué hace (buffers múltiples)

Flint ya no está limitado a un archivo por proceso: `Ctrl+O` abre otro archivo en un buffer nuevo, `Ctrl+PageDown`/`Ctrl+PageUp` cambian de buffer, y `Ctrl+W` cierra el activo. Cada buffer tiene su propio `Editor` completo — rope, cursor/selección, historial de deshacer/rehacer, resaltado de sintaxis y estado de "sin guardar" — totalmente independiente de los demás; no hay estado compartido entre archivos abiertos salvo el que corresponde compartir (portapapeles, tema, perfil de teclado, servidor LSP).

- Con dos o más buffers abiertos aparece una barra de pestañas debajo del título, con el activo resaltado y un `•` en los que tienen cambios sin guardar.
- `Ctrl+Q` (o `Ctrl+W`) cierra el buffer activo preguntando primero si hay cambios sin guardar; el proceso solo termina al cerrar el último — con más de un buffer abierto, el mensaje de confirmación dice "cerrar este buffer" en vez de "salir", para no generar confusión sobre qué se pierde.
- **Un único servidor LSP por lenguaje, no uno por archivo**: si se abren varios `.rs`, todos comparten el mismo proceso `rust-analyzer` — cada buffer abre su propio documento (`textDocument/didOpen` con su URI) contra ese servidor compartido, se sincroniza (`didChange`) y se cierra (`didClose`) de forma independiente. Los diagnósticos que llegan del servidor se enrutan al buffer correcto por URI, no siempre al activo — importante porque el servidor puede terminar de analizar un archivo después de que se cambió a otra pestaña. El servidor solo se ofrece instalar (diálogo interactivo) para el primer archivo que lo necesita al arrancar; un buffer del mismo lenguaje abierto después con `Ctrl+O` se suma automáticamente si el servidor ya está corriendo, y si no, se queda sin LSP (el resaltado de sintaxis no depende de esto).

Se probó a mano con tmux: dos archivos de texto abiertos a la vez, editando cada uno con contenido distinto y confirmando que cambiar de pestaña no mezcla ni pierde el estado de ninguno; cerrar un buffer con cambios sin guardar y responder "no" (se descarta, y el archivo en disco queda intacto); cerrar el único buffer restante con "sí" (guarda y termina el proceso); y los comandos de la paleta ("Abrir archivo…", "Siguiente/anterior buffer", "Cerrar buffer") apareciendo y funcionando igual que los atajos de teclado.

## Qué hace (empaquetado)

Hasta ahora la única forma de instalar Flint era `cargo install --path .` — que funciona, pero exige tener Rust instalado en la máquina donde se usa. `packaging/build-deb.sh` arma un paquete `.deb` real (Debian/Ubuntu/Kali y derivados), pensado para instalarse en una máquina que **no** tiene el toolchain de Rust:

- Compila en modo release, hace `strip` del binario, y arma el árbol del paquete (`/usr/bin/flint`, y `README.md`/`MANUAL.md`/`theme.example.toml`/`plugins/ejemplo.lua` de referencia en `/usr/share/doc/flint/` y `/usr/share/flint/plugins/`).
- Las dependencias (`Depends:` del `control`) no están adivinadas a mano — se calculan con `dpkg-shlibdeps` a partir del binario real. Terminan siendo solo `libc6` y `libgcc-s1`: `arboard` (el portapapeles del sistema) habla X11 por protocolo puro vía `x11rb`, sin enlazar `libX11.so`, así que no suma ninguna dependencia de sistema de ventanas.
- `dpkg-shlibdeps` necesita un `debian/control` para correr (aunque sea fuera de un build con `debhelper`) — el script arma uno mínimo y descartable solo para esa consulta, y lo borra enseguida; no queda un directorio `debian/` sobrante en el repo.

Se probó de punta a punta: `.deb` construido y su `control`/contenido inspeccionados con `dpkg-deb --info`/`--contents`; extraído sin privilegios de root (`dpkg-deb --extract`) para confirmar que el binario resultante corre igual que el compilado directo — abre un archivo, lo edita y sale, probado con tmux —, sin depender de nada del árbol de `cargo build` (ni rutas relativas al repo). La instalación real con `sudo apt install ./flint_*.deb` se hizo después, a mano: el paquete instala en `/usr/bin/flint` y corre sin el árbol de `cargo` presente.

Lo que queda afuera de este alcance: Homebrew y binarios adjuntos a una release de GitHub. El repositorio público ya existe, así que dejaron de estar bloqueados — son el próximo paso natural de distribución, no un imposible.

## Qué hace (auto-indentación, búsqueda con regex, ajuste de línea)

- **Auto-indentación**: `Enter` (en cualquier capa, y `o`/`O` en NORMAL, que usan la misma función) copia el espacio en blanco inicial de la línea donde estaba el cursor — no es "inteligente" con llaves/paréntesis (eso depende del lenguaje y queda para otra fase), solo mantiene el nivel actual. Cada cursor de una edición multi-cursor calcula la indentación de su propia línea. Probado a mano: `Enter` después de una línea indentada con 4 espacios en Python, la línea nueva arranca ya indentada igual.
- **Anchos de pantalla reales (tabuladores, CJK, emoji, acentos combinantes)**: el buffer cuenta caracteres y la terminal cuenta columnas, y no son lo mismo: un `\t` ocupa hasta la próxima parada de tabulación, un `日` o un `😀` ocupan dos columnas, y una marca combinante (`e` + `◌́` = `é`) ocupa cero. Toda la capa de dibujo —cursor, scroll horizontal, ajuste de línea, clics del mouse y los rangos de resaltado/selección/diagnóstico— trabaja en columnas de pantalla, traducidas en un solo lugar (`char_display_width`), que mide con `unicode-width`: **la misma tabla que usa ratatui para medir lo que dibuja**, que es lo que garantiza que el cursor caiga donde está el texto. Un carácter ancho partido por el borde de la ventana se dibuja como un espacio (medio glifo no se puede dibujar, y ocupar su columna mantiene alineado lo que sigue) y un clic sobre su mitad derecha pone el cursor al principio del carácter, no dentro. Probado a mano: `日本語x` con el cursor al final cae en la columna 12 (antes caía en la 9); `emoji 😀 fin` renderiza `fin` exactamente donde el modelo dice; `café` escrito como `e`+combinante no pierde la tilde; y con scroll horizontal cortando un `日` al medio, el resto de la línea sigue alineado.
- **Indentar y des-indentar un bloque**: con una selección que abarca varias líneas, `Tab` indenta todas esas líneas en vez de reemplazarlas por un tabulador — que es lo que haría cualquier otra inserción, y nunca es lo que se quiso (antes de esto, `Tab` con dos líneas seleccionadas borraba las dos). La selección sobrevive, así que se puede apretar `Tab` varias veces para subir de nivel. `Shift+Tab` (que la terminal manda como `BackTab`) hace lo contrario en cada línea tocada, con o sin selección: saca un tabulador, o hasta `tab_width` espacios si esa línea se indentó con espacios. Las líneas que ya están pegadas al margen se dejan como están, y si ninguna tenía qué sacar no se toca el buffer: ni se marca sucio ni se gasta un paso de deshacer. Ambas acciones también están en la paleta. `Tab` sin selección, o con una selección dentro de una sola línea, sigue insertando un tabulador como siempre. Probado a mano: indentar dos líneas y repetir para llegar a dos niveles, des-indentar con `Shift+Tab`, deshacer todo con `Ctrl+Z`, sacar 4 de 8 espacios en una línea indentada con espacios, y confirmar que `Shift+Tab` sobre una línea sin indentación no enciende la marca de "sin guardar" en la barra de título.
- **Búsqueda y reemplazo con regex**: "Buscar (regex)…" y "Reemplazar (regex)…" en la paleta de comandos (`Ctrl+P` — sin atajo de teclado dedicado, para no arriesgar un choque con el `Ctrl+F`/`Ctrl+R` literal que ya existía y sigue igual). El reemplazo admite grupos capturados (`$1`, `${nombre}`, sintaxis del crate `regex`). Una regex inválida muestra el error del motor en la barra de estado, sin romper nada. A diferencia de la búsqueda literal (que recorre el rope por trozos, ver más abajo), esta sí vuelca el buffer entero a un `String` — el motor de regex no sabe leer un rope por trozos — pero solo pasa cuando se pide de verdad una búsqueda con regex, no en cada tecla. Probado a mano: `\d+` encuentra la primera corrida de dígitos; `(\w+?)(\d+)` → `$2$1` reemplazando-todo intercambia letras y números (`foo123` → `123foo`); una regex con paréntesis sin cerrar muestra el error sin crashear.
- **Ajuste de línea**: `Ctrl+L` (o "Alternar ajuste de línea" en la paleta) alterna entre scroll horizontal (el comportamiento de siempre, sigue siendo el default) y partir las líneas largas en varias filas de pantalla. La barra de título marca `[wrap]` cuando está activo. El número de línea y la marca de diagnóstico solo aparecen en la primera fila visual de cada línea; las siguientes son continuación, sin numerar (como la mayoría de los editores con wrap). El corte es por carácter, no por palabra — más simple y predecible que buscar el espacio más cercano, y es lo mismo que hace `nano` por defecto. Eso sí, nunca parte un carácter al medio: si el siguiente es ancho (CJK, emoji) y no entra entero, la fila termina antes y baja completo a la siguiente. Los cortes los calcula `wrap_row_starts` en un solo lugar, y de ahí los toman por igual el dibujo, el cálculo de scroll y los clics del mouse — si cada uno los recalculara a su manera, un clic caería en un carácter distinto del que se ve. El mouse mapea clics correctamente incluso en filas de continuación. Con una sola línea más alta que toda la pantalla (rarísimo en la práctica), se muestra desde su principio en vez de desplazarse *dentro* de esa línea — es la única simplificación deliberada del algoritmo de scroll. Probado a mano: activar wrap con líneas de ~150 caracteres en una terminal de 60 columnas, moverse con las flechas a través de las filas partidas, hacer scroll más allá de lo visible, clickear con el mouse en una fila de continuación (con una secuencia SGR cruda) y confirmar que el texto se insertó exactamente ahí, y volver a desactivarlo sin dejar rastros.

## Qué falta a propósito (fases siguientes)

Esto es intencionalmente angosto — nada de esto es un bug, es alcance de una fase posterior:

- Solo Rust tiene servidor LSP conectado; Python/JSON/TOML se quedan en resaltado de sintaxis sin diagnósticos ni autocompletado (harían falta `pylsp`/`pyright` etc., con su propia receta de instalación).
- Los plugins de Lua no pueden tocar el portapapeles ni el registro todavía.
- No hay forma de que un plugin de Lua defina su propia paleta de colores de sintaxis para un lenguaje que Flint no conozca — necesitaría poder cargar una gramática de tree-sitter en tiempo de ejecución (las que trae Flint hoy están compiladas de fábrica, como crates de Rust), que es un motor de carga dinámica aparte, no una extensión chica.
- Los plugins de Lua no pueden asignar atajos propios ni tocar la selección — ver la nota de la Fase 3 arriba.
- Resaltado de sintaxis: sigue siendo `O(n)` por recálculo — una reescritura de tree-sitter con parseo incremental de verdad (`Tree::edit` + reutilizar el árbol viejo) queda deliberadamente afuera: hacer mal el cálculo de posiciones del edit corrompe el árbol en *cualquier* archivo, no solo en los grandes, así que el riesgo no es proporcional a esta fase. Lo que sí se hizo fue no pagar ese costo en cada tecla: un margen de 120ms agrupa una ráfaga de tipeo en un solo recálculo al final, en vez de uno por letra.
- Archivos muy grandes: la búsqueda y el reemplazo siguen siendo `O(n)` en tiempo (inevitable — hay que leer el archivo para encontrar algo en él), pero ya no copian el buffer entero a memoria de más de lo necesario: la búsqueda recorre el rope directamente con una ventana deslizante en vez de volcarlo a un `String` primero, y reemplazar-todo cuenta y arma el resultado en una sola pasada en vez de dos separadas.
- El tema no detecta si la terminal está en claro u oscuro — no hay forma portable y confiable de preguntárselo a la terminal (la consulta OSC 11 que algunas soportan no la implementan todas, y menos todavía atravesando `tmux`/`screen`); en vez de una detección que funcione a veces y se cuelgue esperando respuesta otras, queda a mano en `theme.toml` (que sí se recarga en caliente, ver abajo).

Ya resuelto (dejó de estar en esta lista):

- **CRLF**: el editor ahora detecta la convención de fin de línea del archivo al abrirlo (mirando el primer salto de línea) y una línea *nueva* (`Enter`) usa esa misma convención — ya no se mezclan `\r\n` existentes con `\n` sueltos recién creados. La barra de título marca `CRLF` cuando corresponde (LF, al ser el default, no se marca). Probado a mano: archivo `\r\n`, `Enter` en medio del archivo, guardado, y verificación byte a byte (`od -c`) de que el salto nuevo también es `\r\n`.
- **"Guardar como" con otra extensión**: ahora recalcula resaltado de sintaxis (y conexión LSP, si corresponde) en el momento, sin reabrir Flint — cubre tanto cambiar de extensión como el caso de un buffer que nunca tuvo nombre (y por lo tanto nunca tuvo lenguaje detectado) hasta el primer guardado. Probado a mano: buffer sin nombre con código Rust, `Ctrl+S` → `nuevo.rs`, y el resaltado aparece de inmediato (confirmado tanto por el mensaje de estado como por los colores ANSI reales de la captura de pantalla).
- **Sincronización LSP incremental**: cuando el servidor la anuncia en `initialize` (`textDocumentSync.change == 2` — rust-analyzer lo hace), Flint manda deltas de rango (`textDocument/didChange` con `range`+`text`) en vez del documento completo, para el caso común de un solo cursor editando. Con multi-cursor, deshacer/rehacer o reemplazar-todo — donde varias ediciones "simultáneas" no calzan con el modelo secuencial que exige el protocolo — cae a mandar el documento completo, que sigue siendo siempre correcto. De paso se encontró y arregló un bug real, más de fondo que el propio sync incremental: las columnas que usa LSP son unidades UTF-16, no caracteres Unicode (lo que usa `Position` puertas adentro) — no son lo mismo con texto no-ASCII antes de la posición. Ahora hay conversión en los dos sentidos (mandar posiciones al servidor, e interpretar las que manda de vuelta en diagnósticos). Probado a mano contra `rust-analyzer` de verdad: tipeo letra por letra (varios deltas incrementales separados) generando un error real, diagnóstico correcto; `Ctrl+Z` disparando el fallback a sync completo y volviendo todo a un estado consistente; y el estado "· rust-analyzer listo (sync incremental)" confirmando que el servidor lo anunció.
- **Autocompletado que filtra**: el popup arranca ya filtrado por el identificador tipeado antes de pedirlo (`s.pu` + autocompletar filtra por "pu", no vacío) y sigue filtrando mientras se sigue escribiendo con el popup abierto — mismo mecanismo de puntaje difuso que la paleta de comandos. Al aceptar una sugerencia se borra todo lo tipeado desde que se abrió el popup y se inserta el texto definitivo de la entrada elegida (usa `insertText` del servidor cuando lo da y no es un snippet — Flint todavía no expande `$0`/`${1:nombre}`, ahí cae a `label`) — así nunca queda texto duplicado ni sobrante, sin importar qué tan aproximado fue el filtro tipeado. Probado a mano contra `rust-analyzer`: `s.pu` + autocompletar → lista filtrada a `push`/`push_str`; `Enter` → `s.push_str`, sin restos.
- **Diagnósticos por columna exacta**: además de la marca en el margen (que sigue mostrando la severidad de toda la línea), ahora se subraya con color el rango preciso que reportó el servidor — convertido de UTF-16 a caracteres antes de guardarlo. Probado a mano: un error de método inexistente en `rust-analyzer` subraya solo esas dos letras, no la línea completa (confirmado con la salida ANSI cruda: `\e[4m` + `\e[58;2;r;g;bm` sobre el rango exacto).
- **`select_word` (`w`) con límites de palabra Unicode reales**: usa el algoritmo estándar (UAX #29, vía el crate `unicode-segmentation`) en vez de alfanumérico+`_` carácter por carácter — agrupa bien contracciones (`don't` es una sola palabra), acentos e identificadores con `_`. Sigue mirando solo la línea actual y arrancando desde el cursor hacia adelante (eso no cambió, es un comportamiento distinto al de "límites Unicode" y no se tocó). Probado a mano: `don't café_test naïve` — las tres se seleccionan enteras como una sola palabra cada una.
- **Barra de pestañas clickeable**: un clic izquierdo en una pestaña cambia a ese buffer, con la misma detección de columna que usa `draw_tabs` para dibujarlas (un solo cálculo compartido, no dos que se puedan desincronizar). Antes solo se podía cambiar de buffer por teclado o desde la paleta. Probado a mano con una secuencia SGR de mouse cruda por tmux, clickeando cada pestaña.
- **Recarga en caliente de `theme.toml`**: si el archivo cambia en disco mientras Flint está corriendo, se detecta (revisando la fecha de modificación una vez por segundo, no el contenido en cada vuelta del loop) y se recarga solo — ya no hace falta reabrir Flint para ver un cambio de tema. Probado a mano: cambiar colores del `.toml` con Flint abierto y ver el cambio reflejado en la salida ANSI real, sin reiniciar el proceso.
