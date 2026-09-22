#!/usr/bin/env bash
# Arma dist/flint-<versión>-<plataforma>.tar.gz: el binario suelto, la
# documentación, el tema de ejemplo y los plugins. Es lo que se publica para
# todo lo que no instala un .deb.
#
# Uso: packaging/tarball.sh <versión> <plataforma>
set -euo pipefail

VERSION="$1"
PLATAFORMA="$2"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
source packaging/reproducible.sh

cargo build --release --locked

dir="flint-${VERSION}-${PLATAFORMA}"
rm -rf "dist/$dir"
mkdir -p "dist/$dir"
cp "$CARGO_TARGET_DIR/release/flint" "dist/$dir/"
strip "dist/$dir/flint"
cp README.md MANUAL.md LICENSE theme.example.toml config.example.toml "dist/$dir/"
cp -a plugins "dist/$dir/"
find "dist/$dir" -type d -exec chmod 755 {} +
find "dist/$dir" -type f -exec chmod 644 {} +
chmod 755 "dist/$dir/flint"

# Mismo orden de archivos, mismas fechas y mismo dueño en cualquier máquina;
# `gzip -n` no guarda nombre ni fecha. Son opciones de GNU tar: el tar de
# macOS no las tiene, así que ahí se usa gtar si está, y si no el tar.gz sale
# bien pero no reproducible.
TAR=tar
command -v gtar >/dev/null 2>&1 && TAR=gtar
if "$TAR" --version 2>/dev/null | grep -q GNU; then
    "$TAR" --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
        -cf - -C dist "$dir" | gzip -n -9 > "dist/$dir.tar.gz"
else
    echo "!! Sin GNU tar: el tar.gz no va a ser reproducible" >&2
    tar -czf "dist/$dir.tar.gz" -C dist "$dir"
fi
rm -rf "dist/$dir"
echo "Listo: dist/$dir.tar.gz"
