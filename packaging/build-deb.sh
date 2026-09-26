#!/usr/bin/env bash
# Construye un paquete .deb de Flint listo para instalar con `dpkg -i` o
# `apt install ./flint_*.deb` — a diferencia de `cargo install --path .`,
# la máquina donde se INSTALA no necesita tener Rust ni cargo, solo la
# máquina donde se CONSTRUYE.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source packaging/reproducible.sh

PKG_NAME="flint"
VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
ARCH=$(dpkg --print-architecture)
OUT_DIR="$ROOT_DIR/target/deb"
PKG_DIR="$OUT_DIR/${PKG_NAME}_${VERSION}_${ARCH}"

echo "==> Compilando en modo release…"
cargo build --release --locked

echo "==> Armando el árbol del paquete…"
rm -rf "$PKG_DIR"
mkdir -p \
    "$PKG_DIR/DEBIAN" \
    "$PKG_DIR/usr/bin" \
    "$PKG_DIR/usr/share/doc/flint" \
    "$PKG_DIR/usr/share/flint/plugins"

install -m 755 "$CARGO_TARGET_DIR/release/flint" "$PKG_DIR/usr/bin/flint"
strip --strip-unneeded "$PKG_DIR/usr/bin/flint"

install -m 644 README.md MANUAL.md theme.example.toml config.example.toml "$PKG_DIR/usr/share/doc/flint/"
# La MIT pide que el aviso de copyright acompañe cada copia; en Debian ese
# aviso vive en /usr/share/doc/<paquete>/copyright.
install -m 644 LICENSE "$PKG_DIR/usr/share/doc/flint/copyright"
cp -a plugins/. "$PKG_DIR/usr/share/flint/plugins/"
# Los permisos no pueden depender del umask de quien hizo el checkout.
find "$PKG_DIR" -type d -exec chmod 755 {} +
find "$PKG_DIR/usr/share" -type f -exec chmod 644 {} +

# Dependencias reales del binario (libc/libgcc, ninguna más — arboard habla
# X11 por protocolo puro, sin libX11.so de por medio), calculadas del ELF con
# `dpkg-shlibdeps` en vez de adivinadas a mano. Esa herramienta solo corre
# dentro de un árbol de paquete Debian de verdad (busca `debian/control`), así
# que se arma uno mínimo y descartable nada más para esta consulta.
FAKE_DEBIAN="$ROOT_DIR/debian"
rm -rf "$FAKE_DEBIAN"
mkdir -p "$FAKE_DEBIAN"
cat > "$FAKE_DEBIAN/control" <<'EOF'
Source: flint
Section: editors
Priority: optional
Maintainer: Next Step Technology SpA <nextsteptechnology2026@gmail.com>

Package: flint
Architecture: any
Depends: ${shlibs:Depends}
Description: placeholder
 placeholder para que dpkg-shlibdeps pueda calcular las dependencias reales.
EOF
DEPENDS=$(dpkg-shlibdeps -O -e "$PKG_DIR/usr/bin/flint" 2>/dev/null \
    | sed -n 's/^shlibs:Depends=//p')
rm -rf "$FAKE_DEBIAN"
if [ -z "$DEPENDS" ]; then
    echo "!! No se pudieron calcular las dependencias del binario — abortando." >&2
    exit 1
fi

cat > "$PKG_DIR/DEBIAN/control" <<EOF
Package: $PKG_NAME
Version: $VERSION
Section: editors
Priority: optional
Architecture: $ARCH
Depends: $DEPENDS
Maintainer: Next Step Technology SpA <nextsteptechnology2026@gmail.com>
Description: Editor de terminal con capa modal opcional, LSP y multi-cursor
 Flint arranca en modo directo (paridad con nano) y suma, encima, una capa
 modal opcional estilo Kakoune/Helix (F2), resaltado de sintaxis con
 tree-sitter, cliente LSP (diagnosticos, autocompletado, ir a la definicion,
 renombrar, formatear), selecciones multi-cursor, temas configurables,
 perfiles de teclado remapeables (Flint/Vim/Emacs) y plugins en Lua.
 .
 El plugin de ejemplo queda en /usr/share/flint/plugins/, y el manual, el
 tema y la configuracion de ejemplo en /usr/share/doc/flint/.
EOF

# Todas las fechas, la del commit. dpkg-deb con SOURCE_DATE_EPOCH solo baja
# las más nuevas que el commit: una más vieja (un archivo que no cambió desde
# hace días en esta copia del repositorio) se colaría tal cual, y el .deb
# armado acá ya no daría los mismos bytes que el del runner.
find "$PKG_DIR" -exec touch -h -d "@$SOURCE_DATE_EPOCH" {} +

echo "==> Construyendo el .deb…"
DEB_PATH="$OUT_DIR/${PKG_NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$PKG_DIR" "$DEB_PATH"

echo "==> Verificando el paquete con lintian (si está disponible)…"
if command -v lintian >/dev/null 2>&1; then
    lintian "$DEB_PATH" || true
else
    echo "    (lintian no está instalado — se omite el chequeo, no es obligatorio)"
fi

# apt lee el paquete como el usuario _apt, sin privilegios: si algún
# directorio del camino no deja pasar a "otros" (un home en 700, lo normal
# en Kali), apt dice "fichero no admitido". Ahí se instala desde /tmp.
LEGIBLE=si
dir="$OUT_DIR"
while [ "$dir" != "/" ]; do
    if [ "$(( 0$(stat -c %a "$dir") & 1 ))" -eq 0 ]; then
        LEGIBLE=no
        break
    fi
    dir=$(dirname "$dir")
done

echo
echo "Listo: $DEB_PATH"
if [ "$LEGIBLE" = si ]; then
    echo "Instalar con:    sudo apt install $DEB_PATH"
else
    echo "Instalar con (apt no puede leer $dir, por eso se pasa por /tmp):"
    echo "    cp $DEB_PATH /tmp/"
    echo "    sudo apt install /tmp/$(basename "$DEB_PATH")"
fi
echo "Desinstalar con: sudo apt remove flint"
