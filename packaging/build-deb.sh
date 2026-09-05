#!/usr/bin/env bash
# Construye un paquete .deb de Flint listo para instalar con `dpkg -i` o
# `apt install ./flint_*.deb` — a diferencia de `cargo install --path .`,
# la máquina donde se INSTALA no necesita tener Rust ni cargo, solo la
# máquina donde se CONSTRUYE.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PKG_NAME="flint"
VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
ARCH=$(dpkg --print-architecture)
OUT_DIR="$ROOT_DIR/target/deb"
PKG_DIR="$OUT_DIR/${PKG_NAME}_${VERSION}_${ARCH}"

echo "==> Compilando en modo release…"
cargo build --release

echo "==> Armando el árbol del paquete…"
rm -rf "$PKG_DIR"
mkdir -p \
    "$PKG_DIR/DEBIAN" \
    "$PKG_DIR/usr/bin" \
    "$PKG_DIR/usr/share/doc/flint" \
    "$PKG_DIR/usr/share/flint/plugins"

install -m 755 target/release/flint "$PKG_DIR/usr/bin/flint"
strip --strip-unneeded "$PKG_DIR/usr/bin/flint"

install -m 644 README.md MANUAL.md theme.example.toml "$PKG_DIR/usr/share/doc/flint/"
cp -a plugins/. "$PKG_DIR/usr/share/flint/plugins/"

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
Maintainer: Flint <nextsteptechnology2026@gmail.com>

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
Maintainer: Flint <nextsteptechnology2026@gmail.com>
Description: Editor de terminal con capa modal opcional, LSP y multi-cursor
 Flint arranca en modo directo (paridad con nano) y suma, encima, una capa
 modal opcional estilo Kakoune/Helix (F2), resaltado de sintaxis y LSP real
 via tree-sitter, selecciones multi-cursor, temas configurables, perfiles
 de teclado remapeables (Flint/Vim/Emacs) y soporte para plugins en Lua.
 .
 Los plugins de ejemplo y el tema de ejemplo quedan instalados en
 /usr/share/flint/ y /usr/share/doc/flint/ como referencia.
EOF

echo "==> Construyendo el .deb…"
DEB_PATH="$OUT_DIR/${PKG_NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$PKG_DIR" "$DEB_PATH"

echo "==> Verificando el paquete con lintian (si está disponible)…"
if command -v lintian >/dev/null 2>&1; then
    lintian "$DEB_PATH" || true
else
    echo "    (lintian no está instalado — se omite el chequeo, no es obligatorio)"
fi

echo
echo "Listo: $DEB_PATH"
echo "Instalar con:   sudo apt install ./$(basename "$DEB_PATH")"
echo "Desinstalar con: sudo apt remove flint"
