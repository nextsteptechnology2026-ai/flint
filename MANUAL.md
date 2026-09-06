# Manual de Flint

Referencia de uso — todos los atajos, comandos y opciones de configuración. Para la historia de cómo se construyó cada parte (y qué se probó) ver el `README.md`.

## Instalación y arranque

**Opción 1 — con Rust instalado**, compila e instala en un paso:

```sh
cd flint
cargo install --path .        # instala el binario `flint` en ~/.cargo/bin
```

**Opción 2 — paquete `.deb`** (Debian/Ubuntu/Kali y derivados), para instalar en una máquina que **no** tiene Rust:

```sh
cd flint
./packaging/build-deb.sh                        # construye (necesita Rust acá)
sudo apt install ./target/deb/flint_*_amd64.deb  # instala (acá no hace falta Rust)
sudo apt remove flint                            # desinstalar
```

**Opción 3 — descargar un binario ya compilado** de la página de releases (https://github.com/nextsteptechnology2026-ai/flint/releases): el `.deb` para Debian y derivadas, o el `.tar.gz` para cualquier otra distro de Linux x86_64 (trae el binario, la documentación, el tema de ejemplo y los plugins). El `sha256sums.txt` de la misma release permite verificarlos.

Para publicar una release nueva: subir `version` en `Cargo.toml`, commitear, y empujar una etiqueta `vX.Y.Z` — el workflow `release.yml` compila, corre los tests, arma los dos paquetes y los adjunta solo si la etiqueta coincide con `Cargo.toml`.

El script del `.deb` calcula las dependencias reales del binario (`dpkg-shlibdeps`, no una lista a mano) y deja el `.deb` en `target/deb/`. Instala `flint` en `/usr/bin/`, y el `README.md`/`MANUAL.md`/`theme.example.toml`/`plugins/ejemplo.lua` de referencia en `/usr/share/doc/flint/` y `/usr/share/flint/plugins/`.

```sh
flint                                  # buffer sin nombre
flint archivo.txt                      # abre (o crea) archivo.txt
flint --profile vim archivo.rs         # con el perfil de teclado Vim
flint --profile emacs archivo.rs       # o Emacs
flint --theme mi_tema.toml archivo.rs  # con un tema de colores propio
flint --help                           # ayuda rápida en la terminal
flint --version                        # versión instalada
```

## El modelo de edición

Flint arranca siempre en **modo directo** — como `nano`, sin nada que aprender antes de poder escribir. `F2` activa una **capa modal opcional** (selección→acción, al estilo Kakoune/Helix) encima de eso; `F2` de nuevo la apaga. La capa modal tiene dos sub-modos: **NORMAL** (las teclas seleccionan o actúan) e **INSERT** (tipeo literal, igual que el modo directo).

Los atajos con `Ctrl` (guardar, buscar, deshacer, diagnósticos, paleta de comandos...) funcionan igual en las tres — nunca quedan atrapados detrás de la capa modal.

## Atajos — modo directo

| Tecla | Acción |
|---|---|
| `Ctrl+S` | Guardar (pide nombre si el buffer no tiene uno) |
| `Ctrl+Q` | Salir (pregunta si hay cambios sin guardar) |
| `Ctrl+F` | Buscar — Enter con el campo vacío repite la última búsqueda |
| `Ctrl+R` | Reemplazar (pide texto a buscar y con qué reemplazarlo) |
| `Ctrl+Z` / `Ctrl+Y` | Deshacer / Rehacer |
| `Ctrl+P` | Paleta de comandos |
| `Ctrl+Espacio` | Autocompletar: con servidor LSP conectado, sus sugerencias; si no hay (o el servidor no tiene nada que ofrecer), las palabras que ya escribiste en el archivo |
| `Ctrl+G` | Saltar al siguiente diagnóstico |
| `Ctrl+D` | Agregar un cursor en la siguiente aparición del texto seleccionado |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+V` | Copiar / Cortar / Pegar — con el portapapeles del sistema |
| `Ctrl+O` | Abrir un archivo en un buffer nuevo (pide la ruta) |
| `Ctrl+W` | Cerrar el buffer activo (pregunta si hay cambios sin guardar) |
| `Ctrl+PageDown` / `Ctrl+PageUp` | Siguiente / anterior buffer |
| `Ctrl+L` | Alternar ajuste de línea (partir líneas largas en vez de scroll horizontal) |
| `Ctrl+E` | Vista previa de Markdown (solo en `.md`): muestra el documento formateado, de solo lectura |
| `F2` | Activar/desactivar la capa modal |
| `Esc` | Si hay cursores extra, los descarta |
| Flechas / `Home` / `End` / `PageUp` / `PageDown` | Mover el cursor |
| `Shift` + cualquiera de las anteriores | Mover extendiendo la selección |
| Cualquier carácter | Se inserta en el cursor (y reemplaza la selección, si hay una) |
| `Enter` | Línea nueva, con auto-indentación (copia el espacio en blanco inicial de la línea actual) |
| `Tab` | Inserta un tabulador. Con una selección que abarca varias líneas, indenta todas esas líneas (y las deja seleccionadas, así se puede repetir) |
| `Shift+Tab` | Saca un nivel de indentación de cada línea tocada: un tabulador, o hasta `tab_width` espacios si la línea usa espacios |
| `Backspace` / `Delete` | Los de siempre |

La paleta de comandos (`Ctrl+P`) también tiene "Buscar (regex)…" y "Reemplazar (regex)…" — mismo flujo que `Ctrl+F`/`Ctrl+R`, pero el texto se interpreta como expresión regular (sintaxis del crate `regex` de Rust) en vez de texto literal; el reemplazo admite grupos capturados (`$1`, `${nombre}`). No tienen atajo de teclado propio, para no arriesgar un choque con la búsqueda literal.

## Atajos — capa modal, NORMAL

| Tecla | Acción |
|---|---|
| `h`/`j`/`k`/`l` o flechas | Mover (colapsa la selección) |
| `Shift` + flechas | Mover extendiendo la selección |
| `w` | Seleccionar la palabra bajo/después del cursor (límites de palabra Unicode reales — `don't` es una sola palabra, `café_test` también) |
| `x` | Seleccionar la línea actual (repetida, extiende una línea más) |
| `n` | Expandir la selección al nodo de sintaxis que la contiene (repetida, sube un nivel del árbol) |
| `d` | Cortar: borra el rango si hay selección (si no, el carácter siguiente) y lo manda al portapapeles del sistema — se puede pegar después con `p`, o en cualquier otra aplicación |
| `c` | Cambiar: corta la selección (si hay, igual que `d`) y entra a INSERT |
| `y` | Copiar la selección al portapapeles del sistema |
| `p` | Pegar — el portapapeles del sistema si hay algo ahí, si no el registro interno |
| `i` / `a` | Entrar a INSERT al inicio / fin de la selección |
| `I` / `A` | Entrar a INSERT al inicio / fin de la línea |
| `o` / `O` | Abrir una línea nueva abajo / arriba (con auto-indentación) y entrar a INSERT |
| `u` / `U` | Deshacer / Rehacer (sin `Ctrl`) |
| `Esc` | Descarta cursores extra; si no hay, deselecciona |
| `Ctrl+D` | Agregar un cursor en la siguiente aparición (igual que en modo directo) |

Todas las acciones de esta tabla actúan sobre **todas** las selecciones activas a la vez cuando hay multi-cursor, no solo sobre la primaria.

## Atajos — capa modal, INSERT

Tipeo literal, igual que el modo directo. `Esc` vuelve a NORMAL (no a modo directo — `F2` es lo que apaga la capa modal entera).

## Mouse

| Acción | Efecto |
|---|---|
| Clic | Mueve el cursor ahí; si había cursores extra, los descarta |
| Arrastrar | Selecciona (extiende la selección de la primaria) |
| `Alt`+clic | Agrega un cursor nuevo donde se hace clic, sin tocar los que ya había |
| Rueda | Desplaza la vista |

El mouse alimenta la misma selección que el teclado — se puede arrastrar para seleccionar y después presionar `d` en NORMAL para cortar esa selección (y `p` en otra parte del archivo para pegarla), por ejemplo.

## Multi-cursor

- `Ctrl+D` (en cualquier capa) toma la selección más reciente con texto y agrega una selección nueva sobre la siguiente aparición de ese mismo texto — repetirlo va sumando cursores, saltando los que ya están y dando la vuelta al llegar al final.
- `Alt`+clic agrega un cursor suelto (sin rango) donde se hace clic.
- Escribir, `Enter`, `Backspace`, `Delete` y las acciones de NORMAL (`d`/`c`/`y`/`p`/`i`/`a`/`o`/`O`/`n`) actúan en todos los cursores a la vez, como una sola edición — un solo `Ctrl+Z` deshace todo el grupo.
- `Esc` descarta los cursores extra y vuelve a uno solo.
- La barra de título muestra `×N cursores` cuando hay más de uno; las selecciones con rango se ven en el color de selección, los cursores sueltos como un carácter resaltado en un color distinto (la terminal solo puede mostrar un cursor real parpadeando, el de la primaria).

## Portapapeles

Copiar/cortar/pegar hablan con el portapapeles del sistema operativo — no un registro que solo Flint puede leer. Lo que copiás en Flint se puede pegar en cualquier otra aplicación, y lo que copiás afuera se puede pegar en Flint.

- **Modo directo**: `Ctrl+C` copia, `Ctrl+X` corta, `Ctrl+V` pega.
- **Capa modal (NORMAL)**: `y`/`d`/`c`/`p` hacen lo mismo (ver la tabla de arriba).
- Al pegar, si hay algo en el portapapeles del sistema se usa eso; si no está disponible, se usa un registro interno en memoria — copiar/cortar/pegar dentro de Flint nunca dejan de funcionar, aunque no haya portapapeles del sistema accesible (por ejemplo, sin servidor gráfico).

## Buffers (varios archivos a la vez)

Flint puede tener varios archivos abiertos al mismo tiempo, cada uno con su propio cursor, selección, historial de deshacer y estado de "sin guardar" — completamente independientes entre sí.

- `Ctrl+O` pide una ruta y la abre en un buffer nuevo, que pasa a ser el activo.
- `Ctrl+PageDown` / `Ctrl+PageUp` (o "Siguiente buffer" / "Buffer anterior" desde la paleta de comandos) cambian de buffer, dando la vuelta en cualquiera de las dos puntas.
- `Ctrl+W` (o "Cerrar buffer" desde la paleta) cierra el buffer activo — pregunta primero si tiene cambios sin guardar. Con un solo buffer abierto, `Ctrl+W` y `Ctrl+Q` hacen lo mismo.
- `Ctrl+Q` cierra buffers de a uno; el proceso solo termina cuando se cierra el último. Con varios abiertos, el mensaje de confirmación dice "cerrar este buffer" en vez de "salir", para que quede claro qué se está por perder.
- Con dos o más buffers abiertos aparece una barra de pestañas (debajo del título) con el nombre de cada archivo — el activo resaltado, y un `•` en los que tienen cambios sin guardar. La barra es clickeable: un clic en una pestaña cambia a ese buffer.

**LSP compartido**: si dos o más buffers son del mismo lenguaje (por ejemplo, varios `.rs`), comparten un único proceso de servidor — no se lanza un `rust-analyzer` nuevo por archivo. El servidor solo se ofrece instalar (con el diálogo interactivo) para el primer archivo que lo necesita al arrancar Flint; un buffer de ese mismo lenguaje abierto después con `Ctrl+O` se suma automáticamente a ese servidor si ya está corriendo, y si no, se queda sin LSP (el resaltado de sintaxis vía tree-sitter no depende de esto y sigue funcionando igual).

## Paleta de comandos (`Ctrl+P`)

Lista todo lo que Flint sabe hacer, con filtro difuso mientras se escribe (no hace falta el nombre exacto ni el orden exacto de las letras). `↑`/`↓` para moverse, `Enter` para ejecutar, `Esc` para cancelar.

Incluye: Guardar, Salir, Buscar (literal y regex), Reemplazar (literal y regex), Deshacer, Rehacer, Saltar al siguiente diagnóstico, Autocompletar, Alternar capa modal, Alternar ajuste de línea, Seleccionar siguiente aparición, Abrir archivo, Siguiente/anterior buffer, Cerrar buffer, cambiar de perfil de teclado (Flint/Vim/Emacs), y cualquier comando que un plugin de Lua haya registrado.

## Perfiles de teclado

Se eligen con `flint --profile <nombre>` al arrancar, o cambiando en caliente desde la paleta de comandos (buscar "perfil").

- **`flint`** (por defecto) — los atajos de las tablas de arriba.
- **`vim`** — igual que Flint, salvo en NORMAL: `u`/`Ctrl+R` deshacen/rehacen como en Vim de verdad, y `x` borra el carácter bajo el cursor en vez de seleccionar la línea. No es una emulación completa: no hay secuencias `dd`/`ciw`, porque el modelo de Flint sigue siendo selección→acción, no verbo+movimiento.
- **`emacs`** — igual que Flint en NORMAL/INSERT (Emacs no es modal, no había una convención propia que imitar ahí), pero en modo directo: `Ctrl+X Ctrl+S` guarda, `Ctrl+X Ctrl+C` sale (el prefijo de dos teclas más reconocido de Emacs — presionar `Ctrl+X` deja la barra de mensaje en "…" esperando la segunda tecla; cualquier otra tecla cancela la secuencia), `Ctrl+S` busca, `Ctrl+G` cancela.

Hoy no hay forma de definir un perfil propio en un archivo — los tres están fijos en el código (`src/keymap.rs`). Es una limitación conocida, no un bug.

## Temas (`theme.toml`)

Se carga desde `~/.config/flint/theme.toml` si existe, o desde la ruta que le pases con `--theme <ruta>` (que manda por encima de la ubicación por defecto). Sin ninguno de los dos, se usa la paleta ámbar de fábrica. Ver `theme.example.toml` para un ejemplo completo (paleta azul/violeta).

**Se recarga en caliente**: si editás y guardás el `theme.toml` en uso mientras Flint está corriendo, el cambio se aplica solo (revisa la fecha de modificación una vez por segundo) — no hace falta reabrir Flint para ver el resultado. Esto no aplica a la paleta de fábrica (no viene de un archivo que se pueda editar).

Ningún campo es obligatorio — lo que falte, o lo que no se pueda interpretar (un color que no sea `"#rrggbb"`), se queda en su valor por defecto sin romper el resto del archivo.

```toml
[colors]
bar_bg = "#14171b"        # fondo de las barras (título/ayuda/mensaje)
bar_fg = "#f2a93b"        # texto de las barras
text_fg = "#d2d0c4"       # texto normal del buffer
dim = "#787c70"           # números de línea, comentarios, texto secundario
selection_bg = "#3a2c14"  # fondo de una selección con rango
selection_fg = "#e9e6da"
secondary_bg = "#5fd6c9"  # cursor secundario sin rango (multi-cursor)
secondary_fg = "#14171b"
popup_bg = "#1c2025"      # fondo de los popups (autocompletar, paleta)
error = "#e06c5a"         # marca de diagnóstico ✖
warning = "#e6b45a"       # marca de diagnóstico ▲
hint = "#7896c8"          # marca de diagnóstico ●

[syntax]
keyword = "#e6963c"
function = "#5fd6c9"
type = "#e6c478"
string = "#96c478"
comment = "#787c70"
number = "#c48cd6"
constant = "#c48cd6"
operator = "#a0a496"
punctuation = "#a0a496"
property = "#d2b06e"
attribute = "#d2b06e"

[appearance]
transparent_bars = false   # true: las barras dejan pasar el fondo real de tu terminal
cursor_style = "block"     # block | bar | underline
cursor_blink = true
tab_width = 4              # columnas que ocupa un tabulador al dibujarse (1 a 16)
```

`tab_width` es solo presentación: el `\t` sigue siendo un único carácter en el archivo, lo que cambia es hasta dónde se estira en pantalla (cada tabulador llega hasta la próxima parada de tabulación, así que su ancho depende de en qué columna empieza). Se aplica en caliente igual que los colores, y a todos los buffers abiertos.

En la misma línea, los caracteres anchos (CJK, emoji) ocupan dos columnas y las marcas combinantes (un acento que se pinta sobre la letra anterior) ninguna. No hay nada que configurar: el cursor, el scroll, el ajuste de línea y los clics del mouse ya cuentan columnas de pantalla, no caracteres.

`transparent_bars` solo afecta a las barras — el área de texto nunca pintó su propio fondo, así que si tu terminal tiene transparencia/blur configurados, ya se ve ahí de por sí.

## Plugins en Lua

Al iniciar, Flint carga todos los `*.lua` que encuentre en estos tres lugares, de mayor a menor precedencia:

| Directorio | Para qué |
|---|---|
| `./plugins` | Relativo a donde arrancás Flint — para desarrollar dentro del repo |
| `~/.config/flint/plugins/` | Los tuyos, junto al `theme.toml` |
| `/usr/share/flint/plugins/` | Los que instala el paquete `.deb` |

Si el mismo nombre de archivo aparece en más de uno, se carga solo el del directorio de mayor precedencia — así tu `ejemplo.lua` reemplaza al del sistema en vez de que corran los dos y el comando aparezca duplicado en la paleta.

Cada comando que un script registre aparece en la paleta de comandos.

```lua
flint.register_command("id-unico", "Etiqueta que se ve en la paleta", function()
    -- lo que haga el comando
end)
```

API disponible dentro de la función de un comando (deliberadamente chica por ahora):

| Función | Qué hace |
|---|---|
| `flint.status(texto)` | Muestra `texto` en la barra de mensaje |
| `flint.insert_text(texto)` | Inserta `texto` en la posición del cursor |
| `flint.filename()` | Devuelve la ruta del archivo abierto (`""` si no tiene nombre) |
| `flint.line_count()` | Devuelve la cantidad de líneas del buffer |

Ver `plugins/ejemplo.lua` para tres comandos reales y completos (insertar la fecha de hoy, mostrar info del archivo, insertar un saludo).

**Lo que un plugin todavía no puede hacer**: asignar sus propios atajos de teclado, leer o mover la selección, ni tocar el buffer más allá de insertar texto en el cursor.

## Autocompletado (LSP)

`Ctrl+Espacio` pide sugerencias al servidor de lenguaje conectado (hoy, `rust-analyzer` para `.rs`).

**Sin servidor de lenguaje** — un `.txt`, un `.md`, un `.py` — completa con las palabras que ya están escritas en el propio archivo, como `Ctrl+N` en Vim. No distingue mayúsculas (`SERV` encuentra `servidor`) y ordena por cercanía al cursor: lo que escribiste tres líneas más arriba aparece antes que algo del otro extremo del archivo, porque es mucho más probable que sea lo que querés repetir. También es lo que se muestra cuando hay servidor pero no devolvió ninguna sugerencia para esa posición: mejor algo que un "sin sugerencias".

- El popup arranca ya filtrado por el identificador tipeado antes de pedirlo — `s.pu` + `Ctrl+Espacio` filtra por "pu" desde el primer instante, no muestra la lista entera sin filtrar.
- Se puede seguir escribiendo con el popup abierto: cada letra se inserta en el documento *y* refina el filtro (mismo puntaje difuso que la paleta de comandos), `Backspace` deshace un carácter del filtro.
- `↑`/`↓` para moverse, `Enter`/`Tab` para aceptar la seleccionada, `Esc` para cancelar el popup (lo tipeado mientras estuvo abierto queda en el documento — `Esc` no lo deshace, `Ctrl+Z` sí).
- Al aceptar, se borra todo lo tipeado desde que se abrió el popup y se inserta el texto definitivo de la entrada elegida — nunca queda texto duplicado ni sobrante, sin importar qué tan aproximado haya sido el filtro tipeado.
- Si el servidor da un `insertText` que no es un snippet, se usa ese; si es un snippet (`$0`, `${1:nombre}`...) se usa el `label` en su lugar — Flint todavía no expande snippets.

## Diagnósticos

Además de la marca en el margen izquierdo (✖ error, ▲ warning, ● info/hint — resume la severidad más alta de toda la línea), el rango exacto que reportó el servidor se subraya con color en el texto. Un error sobre un solo método mal escrito, por ejemplo, subraya solo esas letras, no la línea entera.

`Ctrl+G` salta al siguiente diagnóstico (da la vuelta al llegar al final) y muestra su mensaje en la barra de estado.

## Vista previa de Markdown

`Ctrl+E` (o "Vista previa de Markdown" en la paleta), en un archivo `.md` o `.markdown`, cambia el área de texto por el documento **renderizado**: los `#`, `**` y `[]()` desaparecen y queda el resultado — títulos en negrita, viñetas de verdad, citas con su barra al margen, cercos de código en un recuadro y con su contenido resaltado según el lenguaje que declaren, y los links subrayados con su URL al lado.

Es una vista de **solo lectura** sobre el mismo buffer; editar sigue siendo sobre el texto fuente, que es lo que uno quiere en un `.md` donde la sintaxis *es* el contenido. Mientras está activa:

| Tecla | Acción |
|---|---|
| `↑` / `↓` | Desplazar una fila |
| `PageUp` / `PageDown` | Desplazar una pantalla |
| `Home` / `End` | Principio / final del documento |
| `Esc` o `Ctrl+E` | Volver a editar |

Los atajos con `Ctrl` (guardar, salir, la paleta) siguen funcionando; cualquier otra tecla no hace nada y lo avisa, para no editar a ciegas algo que no se está viendo. El párrafo se reparte según el ancho de la ventana, así que el render no respeta los saltos de línea del fuente: arma los suyos.

## Lenguajes soportados

| Extensión | Resaltado de sintaxis | Servidor LSP |
|---|---|---|
| `.rs` | Sí (tree-sitter) | Sí (`rust-analyzer`, instalación guiada si falta; sincronización incremental si el servidor la anuncia) |
| `.py` | Sí (tree-sitter) | No |
| `.json` | Sí (tree-sitter) | No |
| `.toml` | Sí (tree-sitter) | No |
| `.md` / `.markdown` | Sí (tree-sitter, dos gramáticas: bloques e inline) | No |
| cualquier otra | No — texto plano | No |

Cualquier extensión no reconocida se edita igual de bien, sin resaltado ni autocompletado — es el comportamiento de "editor de rescate" heredado de la paridad con `nano`.

## Más detalles

El `README.md` explica, fase por fase, cómo se construyó cada parte, qué se probó a mano, y la lista completa de limitaciones conocidas (LSP solo para Rust, sin tests automatizados, etc.).
