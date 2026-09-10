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
flint --config mi_config.toml notas.md # con otro archivo de configuración
flint --actions                        # lista los nombres de acción para [keys]
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
| `Ctrl+T` | Buscador difuso de archivos del proyecto — escribí parte del nombre y `Enter` lo abre |
| `Ctrl+K` | Comentar / descomentar las líneas que toca la selección |
| `Ctrl+U` | Empezar a grabar una macro, o terminarla si ya se está grabando |
| `Ctrl+B` | Repetir la última macro grabada |
| `Ctrl+W` | Cerrar el buffer activo (pregunta si hay cambios sin guardar) |
| `Ctrl+PageDown` / `Ctrl+PageUp` | Siguiente / anterior buffer |
| `Ctrl+L` | Alternar ajuste de línea (partir líneas largas en vez de scroll horizontal) |
| `Ctrl+E` | Vista previa de Markdown (solo en `.md`): muestra el documento formateado, de solo lectura |
| `F2` | Activar/desactivar la capa modal |
| `Esc` | Si hay cursores extra, los descarta |
| Flechas / `Home` / `End` / `PageUp` / `PageDown` | Mover el cursor |
| `Shift` + cualquiera de las anteriores | Mover extendiendo la selección |
| Cualquier carácter | Se inserta en el cursor (y reemplaza la selección, si hay una) |
| `Enter` | Línea nueva con auto-indentación: hereda la sangría de la línea actual y suma un nivel si quedaste adentro de algo abierto y sin cerrar. Si el cierre estaba pegado al cursor, baja a su propia línea |
| `Tab` | Inserta un nivel de indentación — un tabulador, o espacios si `indent_with_spaces` está activo para ese archivo. Con una selección que abarca varias líneas, indenta todas esas líneas (y las deja seleccionadas, así se puede repetir) |
| `Shift+Tab` | Saca un nivel de indentación de cada línea tocada: un tabulador, o hasta `tab_width` espacios si la línea usa espacios |
| `Backspace` / `Delete` | Los de siempre. Entre un par vacío (`()` con el cursor en el medio), `Backspace` se lleva los dos |

La paleta de comandos (`Ctrl+P`) también tiene "Buscar (regex)…" y "Reemplazar (regex)…" — mismo flujo que `Ctrl+F`/`Ctrl+R`, pero el texto se interpreta como expresión regular (sintaxis del crate `regex` de Rust) en vez de texto literal; el reemplazo admite grupos capturados (`$1`, `${nombre}`). No tienen atajo de teclado propio, para no arriesgar un choque con la búsqueda literal.

## Atajos — capa modal, NORMAL

| Tecla | Acción |
|---|---|
| `h`/`j`/`k`/`l` o flechas | Mover (colapsa la selección) |
| `Shift` + flechas | Mover extendiendo la selección |
| `w` | Seleccionar la palabra bajo/después del cursor (límites de palabra Unicode reales — `don't` es una sola palabra, `café_test` también) |
| `x` | Seleccionar la línea actual (repetida, extiende una línea más) |
| `n` | Expandir la selección al nodo de sintaxis que la contiene (repetida, sube un nivel del árbol) |
| `m` | Saltar al paréntesis, corchete o llave que hace pareja con el de al lado del cursor |
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
- La barra de mensaje dice a dónde fue lo copiado, que no siempre es el mismo lugar (ver abajo).

**Por SSH, sin servidor gráfico.** El portapapeles del sistema necesita X11 o Wayland; en una sesión remota no hay ninguno de los dos, y ahí Flint usa **OSC 52**: una secuencia de escape que le pide a la *terminal* que guarde el texto. Como la terminal es la de la máquina que tenés adelante, copiar dentro de un Flint que corre en un servidor termina en tu portapapeles local. Es lo que `arboard` (y por lo tanto el modo `system`) no puede hacer.

Se elige con `clipboard` en `[options]`:

| Valor | Qué hace |
|---|---|
| `auto` (por defecto) | El del sistema; si no hay, la terminal por OSC 52 |
| `system` | Solo el del sistema |
| `terminal` | Siempre OSC 52, aunque haya servidor gráfico — lo que se quiere dentro de un contenedor o un `tmux` remoto |
| `internal` | Ninguno de los dos: solo el registro interno de Flint |

Dos límites, dichos de frente: OSC 52 solo sirve para **copiar**. Leer también está en el estándar, pero casi ninguna terminal lo habilita (dejaría que cualquier programa remoto lea lo que copiaste) y la respuesta llegaría mezclada con las teclas, así que pegar usa el registro interno cuando no hay portapapeles del sistema. Y hay un tope de 64 KiB por copia: más que eso lo cortan las terminales, así que Flint avisa en vez de mandar algo que llegaría a medias.

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

Incluye: Guardar, Salir, Buscar (literal y regex), Reemplazar (literal y regex), Deshacer, Rehacer, Saltar al siguiente diagnóstico, Autocompletar, Alternar capa modal, Alternar ajuste de línea, Seleccionar siguiente aparición, Abrir archivo, Abrir archivo del proyecto, Ir a la línea, Comentar/descomentar, Saltar al par, Grabar y repetir macro, Siguiente/anterior buffer, Cerrar buffer, cambiar de perfil de teclado (Flint/Vim/Emacs), y cualquier comando que un plugin de Lua haya registrado.

Cada renglón muestra además el **nombre estable** de su acción (`Guardar · save`): es el mismo que se escribe en `[keys]` en `config.toml`, así que la paleta también sirve de referencia para remapear sin salir del editor — y buscar `save` encuentra `Guardar`.

## Perfiles de teclado

Se eligen con `flint --profile <nombre>` al arrancar, o cambiando en caliente desde la paleta de comandos (buscar "perfil").

- **`flint`** (por defecto) — los atajos de las tablas de arriba.
- **`vim`** — igual que Flint, salvo en NORMAL: `u`/`Ctrl+R` deshacen/rehacen como en Vim de verdad, y `x` borra el carácter bajo el cursor en vez de seleccionar la línea. No es una emulación completa: no hay secuencias `dd`/`ciw`, porque el modelo de Flint sigue siendo selección→acción, no verbo+movimiento.
- **`emacs`** — igual que Flint en NORMAL/INSERT (Emacs no es modal, no había una convención propia que imitar ahí), pero en modo directo: `Ctrl+X Ctrl+S` guarda, `Ctrl+X Ctrl+C` sale (el prefijo de dos teclas más reconocido de Emacs — presionar `Ctrl+X` deja la barra de mensaje en "…" esperando la segunda tecla; cualquier otra tecla cancela la secuencia), `Ctrl+S` busca, `Ctrl+G` cancela.

Los tres perfiles están en el código (`src/keymap.rs`), pero **no hace falta tocarlo para cambiar teclas**: `[keys]` y `[modal_keys]` en `config.toml` se aplican encima del perfil elegido, así que alcanza con nombrar las teclas que querés distintas (ver la sección siguiente).

## Configuración (`config.toml`)

Se carga desde `~/.config/flint/config.toml` si existe, o desde la ruta que le pases con `--config <ruta>`. Todo es opcional: lo que falte se queda en su valor por defecto, y un valor mal escrito se avisa al arrancar y se descarta **solo él** — el resto del archivo se aplica igual. Ver `config.example.toml` en el repositorio para un archivo completo y comentado.

Las banderas de la línea de comandos ganan por encima del archivo, y el archivo por encima de los valores de fábrica.

### `[options]` — lo general

| Clave | Por defecto | Qué hace |
|---|---|---|
| `profile` | `"flint"` | Perfil de teclado: `flint`, `vim` o `emacs` |
| `theme` | — | Ruta del tema; si falta se busca `~/.config/flint/theme.toml` |
| `tab_width` | `4` | Columnas que ocupa un tabulador **en pantalla** (1 a 16) |
| `indent_with_spaces` | `false` | Indentar con espacios en vez de un tabulador. Esto **sí** cambia el archivo |
| `wrap` | `false` | Arrancar con ajuste de línea activado |
| `trim_trailing_whitespace` | `false` | Sacar los espacios del final de cada línea al guardar |
| `auto_close_brackets` | `true` | Cerrar solo `(`, `[`, `{`, `"` y `'` |
| `clipboard` | `"auto"` | A dónde va lo copiado (ver la sección Portapapeles) |

### `[files."patrón"]` — lo mismo, por archivo

Cualquier clave de `[options]` se puede repetir bajo un patrón y vale solo para los archivos que le calcen:

```toml
[files."*.py"]
indent_with_spaces = true
tab_width = 4

[files."src/*.rs"]
tab_width = 2
```

El patrón acepta `*` (cualquier cosa, incluso nada) y `?` (exactamente un carácter). Sin `/` se compara contra el **nombre** del archivo; con `/`, contra la ruta entera tal como se escribió al abrirlo. Si dos patrones alcanzan al mismo archivo gana el más largo, que es el más específico de los dos.

### `[keys]` y `[modal_keys]` — las teclas

`[keys]` es la capa directa (y INSERT, que tipea igual); `[modal_keys]` es la capa modal NORMAL. La clave es la combinación y el valor el nombre de la acción:

```toml
[keys]
"ctrl+k" = "toggle_comment"
"ctrl+w" = "none"          # desatar la tecla

[modal_keys]
"g" = "move_home"
```

Los modificadores son `ctrl` y `shift`; `alt` todavía no se distingue. Sobre una letra, `shift` va en el propio carácter (`"shift+a"` y `"A"` son la misma tecla). Los nombres de tecla especiales son `space`, `tab`, `enter`, `esc`, `backspace`, `delete`, `home`, `end`, `pageup`, `pagedown` y las cuatro flechas.

`flint --actions` lista todos los nombres de acción; la paleta de comandos muestra el de cada renglón. `"none"` desata la tecla a propósito, y se distingue de un nombre mal escrito: lo primero es una decisión, lo segundo un aviso al arrancar.

`F2` no es remapeable: es el único interruptor global fijo, el que prende y apaga la capa modal.

### `[lsp]` — los servidores de lenguaje

```toml
[lsp]
rust = "rust-analyzer"
python = ["pylsp", "--check-parent-process"]
```

La clave es el identificador LSP del lenguaje (`rust`, `python`, `json`, `toml`, `markdown`); el valor, el comando solo o como lista si necesita argumentos. Lo que no esté acá usa el servidor que Flint trae de fábrica para ese lenguaje. Agregar un servidor nuevo es un renglón acá, no un cambio de código.

Flint solo sabe **instalar** `rust-analyzer` (es un componente de `rustup`). Para cualquier otro servidor que falte avisa y sigue sin LSP: adivinar el gestor de paquetes de la máquina sería peor que no ofrecer nada.

**Probado con `pylsp`**, además de `rust-analyzer`. Con `pipx install python-lsp-server` (o `apt install python3-pylsp`) y ese renglón en `[lsp]`, un `.py` arranca el servidor, anuncia sincronización incremental y responde autocompletado de `jedi`. Para que además marque errores hacen falta los linters, que no vienen en el paquete base: `pipx inject python-lsp-server pyflakes pycodestyle`.

## Buscador de archivos (`Ctrl+T`)

Escribí parte del nombre y `Enter` abre el archivo en un buffer nuevo — mismo filtro difuso que la paleta, mismas teclas (`↑`/`↓`, `Enter`, `Esc`).

La raíz se busca sola: desde el directorio del archivo abierto se sube mientras haya una marca de proyecto (`.git`, `Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `Makefile`) y se usa la más alta que la tenga, así que abrir `src/main.rs` ofrece el repositorio entero y no solo `src/`. Sin ninguna marca, la raíz es el directorio del propio archivo — en `/etc/hosts` nadie quiere indexar `/`.

Se saltean los archivos y directorios ocultos y los que nunca se editan a mano (`.git`, `target`, `node_modules`, `.venv`, `venv`, `__pycache__`, `.mypy_cache`), no se siguen enlaces simbólicos (un enlace hacia arriba sería un recorrido infinito) y hay topes de 20.000 archivos y 12 niveles de profundidad. El índice se arma al abrir el buscador, no al arrancar: así no hay que vigilar el disco y la lista siempre está al día.

## Comentarios (`Ctrl+K`)

Comenta o descomenta las líneas que toca la selección, con el token del lenguaje (`//` en Rust, `#` en Python y TOML). En JSON y Markdown no hace nada y lo dice: JSON no admite comentarios y el único de Markdown es el de HTML, que abre y cierra.

Descomenta solo si **todas** las líneas con texto ya estaban comentadas; con una mezcla, comenta todo — que es lo que uno espera al apretar una vez sobre un bloque a medio comentar. El token entra en la sangría *mínima* del bloque, no en la de cada línea, así que la escalera de indentación de adentro se conserva. Las líneas en blanco se saltean.

## Pares de delimitadores

- **Cierre automático**: escribir `(`, `[`, `{`, `"` o `'` pone también el de cierre y deja el cursor en el medio. Se apaga con `auto_close_brackets = false`.
- Escribir el cierre que ya está no lo duplica: se pasa por encima.
- `Backspace` entre un par vacío se lleva los dos.
- No se cierra pegado a texto (solo si lo que sigue es el borde de la línea, un espacio u otro cierre), ni una comilla en medio de una palabra — un apóstrofo en `don't`, o una vida de Rust, no abren una cadena.
- Con varios cursores se escribe el carácter y nada más: cada cursor tiene su propio contexto y adivinar uno solo para todos daría un texto que nadie pidió.
- **El par del cursor se resalta** en las dos puntas, y `m` en la capa modal NORMAL (o "Saltar al par" en la paleta) salta de una a la otra.

Los delimitadores que están adentro de una cadena o de un comentario **no cuentan**: eso lo sabe el resaltado, que sale del árbol de tree-sitter. Un `"("` suelto dentro de un texto no descuadra la cuenta, como sí pasa en los editores que solo cuentan caracteres.

## Auto-indentación

`Enter` hereda la sangría de la línea actual, y suma un nivel si el cursor quedó adentro de un delimitador abierto y sin cerrar. Si además el cierre estaba pegado al cursor —lo normal después de escribir `{` con el cierre automático puesto—, el cierre baja a su propia línea y el cursor queda en el medio:

```
fn main() {|}        →    fn main() {
                              |
                          }
```

En Python, una línea terminada en `:` también abre bloque. En un lenguaje con llaves no, porque ahí un `:` al final es otra cosa.

Los delimitadores que están dentro de una cadena o de un comentario **no cuentan**, igual que en el salto al par: un `"{"` suelto adentro de un texto no sangra la línea siguiente. Es la misma información del árbol de tree-sitter, y es lo que un editor que solo cuenta caracteres no puede distinguir.

Con varios cursores se mantiene el comportamiento de siempre —cada línea nueva copia la sangría de la suya— porque cada cursor tiene su propio contexto y adivinar uno solo para todos daría un texto que nadie pidió.

## Macros (`Ctrl+U`, `Ctrl+B`)

`Ctrl+U` empieza a grabar y `Ctrl+U` de nuevo termina; `Ctrl+B` repite lo grabado. Se graba todo lo que pasa por el teclado — los atajos y también lo tipeado —, así que una macro repite exactamente lo que hiciste.

Una grabación vacía se descarta en vez de pisar la macro anterior con nada. Las dos teclas de macro nunca entran en la grabación: la que la termina quedaría adentro, y repetir empezaría a grabar otra.

## Respaldos y cambios en el disco

**Si otro proceso tocó el archivo** mientras estaba abierto (un `git checkout`, un formateador, un `sed -i`), guardar no lo pisa en silencio: pregunta una vez, y `n` deja el disco como está.

**Cada 8 segundos**, si hay algo sin guardar, Flint deja una copia en `~/.local/share/flint/backups/`. El nombre lleva la ruta absoluta con las barras pasadas a `%`, así que dos archivos que se llaman igual en carpetas distintas no se pisan. Al abrir un archivo que tiene un respaldo así, la barra de mensaje lo avisa con la ruta — Flint **no** restaura solo: leer el respaldo y decidir es tuyo.

El respaldo se borra al guardar (lo de disco y lo de pantalla vuelven a coincidir) y al cerrar el buffer: cerrar es una decisión, no una caída. Los respaldos existen para los cortes de luz, no para deshacer un "salir sin guardar".

## Temas (`theme.toml`)

El tema es solo colores y forma del cursor; todo lo que cambia el comportamiento del editor vive en `config.toml`. La única clave que aparece en los dos es `tab_width`, por compatibilidad con los temas que ya la traían: si `config.toml` la define, esa gana.

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

`Ctrl+Espacio` pide sugerencias al servidor de lenguaje conectado (`rust-analyzer` para `.rs` de fábrica, y cualquier otro que se configure en `[lsp]`).

El texto que se inserta sale del primer campo que mande el servidor, en este orden: `textEdit.newText`, `insertText`, y si no hay ninguno, la etiqueta. Cada servidor prefiere uno distinto y el spec dice que `textEdit` gana. Los snippets (`insertTextFormat: 2`) no se expanden: se inserta la etiqueta, que es texto plano, en vez de dejar `${1:nombre}` escrito en el archivo.

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

`Ctrl+E` (o "Vista previa de Markdown" en la paleta), en un archivo `.md` o `.markdown`, cambia el área de texto por el documento **renderizado**: los `#`, `**` y `[]()` desaparecen y queda el resultado — títulos en negrita, con sangría por nivel y una raya debajo de los dos primeros (una terminal no puede hacer letras más grandes), viñetas de verdad, citas con su barra al margen, cercos de código en un recuadro y con su contenido resaltado según el lenguaje que declaren, y los links subrayados con su URL al lado.

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
| `.py` | Sí (tree-sitter) | Con `python = "pylsp"` en `[lsp]` — probado: diagnósticos, autocompletado y sincronización incremental |
| `.json` | Sí (tree-sitter) | El que se configure en `[lsp]` |
| `.toml` | Sí (tree-sitter) | El que se configure en `[lsp]` (por ejemplo `taplo`) |
| `.md` / `.markdown` | Sí (tree-sitter, dos gramáticas: bloques e inline) | El que se configure en `[lsp]` |
| cualquier otra | No — texto plano | No |

Cualquier extensión no reconocida se edita igual de bien, sin resaltado ni autocompletado — es el comportamiento de "editor de rescate" heredado de la paridad con `nano`.

## Más detalles

El `README.md` explica, fase por fase, cómo se construyó cada parte, qué se probó a mano, y la lista completa de limitaciones conocidas (LSP solo para Rust, sin tests automatizados, etc.).
