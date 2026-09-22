#!/bin/sh
# Instalador de una línea para Flint.
#
#     curl -fsSL https://raw.githubusercontent.com/nextsteptechnology2026-ai/flint/main/packaging/install.sh | sh
#
# Baja el binario ya compilado de la última release, verifica su sha256
# contra el que publicó la release, y lo deja en el PATH. No necesita Rust ni
# compilar nada. Para desinstalar alcanza con borrar el binario: no toca
# nada más del sistema fuera de los directorios que se listan abajo.
#
# Variables que se pueden pasar por delante:
#   FLINT_VERSION=0.2.0   una versión puntual en vez de la última
#   FLINT_PREFIX=~/.local instalar en otro lado (por defecto elige solo)
#
# Es `sh` a propósito, no bash: este script corre en la máquina de otro y no
# se sabe qué tiene.
set -eu

REPO="nextsteptechnology2026-ai/flint"

error() {
    echo "flint: $*" >&2
    exit 1
}

necesita() {
    command -v "$1" >/dev/null 2>&1 || error "hace falta \"$1\" y no está instalado"
}

necesita tar

# curl o wget, el que haya.
bajar() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        error "hace falta curl o wget y no hay ninguno"
    fi
}

# ---------- qué máquina es esta ----------

case "$(uname -s)" in
    Linux) SO=linux ;;
    Darwin) SO=macos ;;
    *) error "no hay binario para $(uname -s); se puede compilar con \"cargo install --git https://github.com/$REPO\"" ;;
esac

case "$(uname -m)" in
    x86_64 | amd64) ARCH=x86_64 ;;
    arm64 | aarch64) ARCH=aarch64 ;;
    *) error "no hay binario para $(uname -m)" ;;
esac

PLATAFORMA="$ARCH-$SO"

# ---------- qué versión ----------

if [ -n "${FLINT_VERSION:-}" ]; then
    VERSION="${FLINT_VERSION#v}"
else
    # La última release, leída de la redirección de /releases/latest. Se usa
    # eso y no la API de GitHub porque la API tiene límite de pedidos por IP
    # sin autenticar, y un instalador no debería pedir credenciales.
    if command -v curl >/dev/null 2>&1; then
        URL_FINAL=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")
    else
        URL_FINAL=$(wget -qS --spider "https://github.com/$REPO/releases/latest" 2>&1 |
            awk '/^  Location: /{u=$2} END{print u}')
    fi
    VERSION=$(printf '%s\n' "$URL_FINAL" | sed -n 's#.*/tag/v\{0,1\}##p')
    [ -n "$VERSION" ] || error "no pude averiguar cuál es la última versión"
fi

NOMBRE="flint-$VERSION-$PLATAFORMA"
BASE="https://github.com/$REPO/releases/download/v$VERSION"

# ---------- dónde instalar ----------

if [ -n "${FLINT_PREFIX:-}" ]; then
    DESTINO="$FLINT_PREFIX/bin"
elif [ -w /usr/local/bin ] 2>/dev/null; then
    DESTINO=/usr/local/bin
else
    # Sin permiso de escritura en /usr/local/bin se instala en el home, que
    # no necesita sudo. Un instalador que pide la contraseña de root cuando
    # nadie se la dio es exactamente lo que no hay que hacer.
    DESTINO="$HOME/.local/bin"
fi

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

echo "flint: bajando $NOMBRE…"
if ! bajar "$BASE/$NOMBRE.tar.gz" "$TMP/$NOMBRE.tar.gz"; then
    # Linux ARM se publica desde la 0.6.0.
    if [ "$PLATAFORMA" = aarch64-linux ]; then
        error "la $VERSION no tiene binario para Linux ARM; se puede compilar con \"cargo install --git https://github.com/$REPO\""
    fi
    error "no pude bajar $BASE/$NOMBRE.tar.gz"
fi

# ---------- verificar ----------

# El sha256 sale del archivo que publica la misma release. No es protección
# contra GitHub, es protección contra una descarga cortada o un espejo
# desactualizado.
if bajar "$BASE/sha256sums.txt" "$TMP/sha256sums.txt" 2>/dev/null; then
    # El nombre puede venir pelado, con "./" adelante o con el directorio en
    # el que se calculó (las releases viejas dicen "dist/…"): se acepta
    # cualquier prefijo que termine en "/", y se ancla el nombre completo
    # para que "flint-0.2.0-x86_64-linux.tar.gz" no matchee contra otro.
    ESPERADO=$(sed -n "s#^\([0-9a-f]\{64\}\)  *\(.*/\)\{0,1\}$NOMBRE\.tar\.gz\$#\1#p" "$TMP/sha256sums.txt" | head -1)
    if [ -n "$ESPERADO" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            REAL=$(sha256sum "$TMP/$NOMBRE.tar.gz" | cut -d' ' -f1)
        elif command -v shasum >/dev/null 2>&1; then
            REAL=$(shasum -a 256 "$TMP/$NOMBRE.tar.gz" | cut -d' ' -f1)
        else
            REAL=""
        fi
        if [ -n "$REAL" ] && [ "$REAL" != "$ESPERADO" ]; then
            error "el sha256 no coincide: esperaba $ESPERADO y bajó $REAL"
        fi
        [ -n "$REAL" ] && echo "flint: sha256 verificado"
    fi
fi

# ---------- instalar ----------

tar -xzf "$TMP/$NOMBRE.tar.gz" -C "$TMP"
mkdir -p "$DESTINO"
install -m 755 "$TMP/$NOMBRE/flint" "$DESTINO/flint" 2>/dev/null ||
    { cp "$TMP/$NOMBRE/flint" "$DESTINO/flint" && chmod 755 "$DESTINO/flint"; }

# Los plugins de ejemplo, donde Flint los busca para el usuario actual.
PLUGINS="${XDG_CONFIG_HOME:-$HOME/.config}/flint/plugins"
if [ -d "$TMP/$NOMBRE/plugins" ] && [ ! -e "$PLUGINS" ]; then
    mkdir -p "$PLUGINS"
    cp "$TMP/$NOMBRE/plugins/"*.lua "$PLUGINS/" 2>/dev/null || true
fi

echo "flint: instalado en $DESTINO/flint"

# Avisar si el destino no está en el PATH, que es la causa número uno de
# "lo instalé y no anda".
case ":$PATH:" in
    *":$DESTINO:"*) ;;
    *)
        echo ""
        echo "  Ojo: $DESTINO no está en tu PATH. Agregá esto a tu ~/.bashrc o ~/.zshrc:"
        echo ""
        echo "      export PATH=\"$DESTINO:\$PATH\""
        echo ""
        ;;
esac

"$DESTINO/flint" --version
