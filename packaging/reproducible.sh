# Se incluye con `source` desde los scripts que arman lo que se publica.
# Deja el entorno listo para que dos compilaciones del mismo commit den los
# mismos bytes, estén donde estén:
#
# - SOURCE_DATE_EPOCH: la fecha del commit, no la de hoy. dpkg-deb la usa
#   para las fechas de los archivos del .deb, y tarball.sh también.
# - Las rutas de la máquina (la del repo y la de ~/.cargo) quedan dentro del
#   binario en los mensajes de pánico y en el C de las gramáticas. Se
#   reemplazan por rutas fijas.
#
# El compilador ya está fijado en rust-toolchain.toml. Lo que queda afuera es
# el enlazador del sistema: para reproducir exactamente un binario publicado,
# hay que compilar en la misma imagen que el runner (ubuntu-latest).

ROOT_DIR="${ROOT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"

export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-$(git -C "$ROOT_DIR" log -1 --format=%ct)}"
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$ROOT_DIR=/flint --remap-path-prefix=$CARGO_DIR=/cargo"
export CFLAGS="${CFLAGS:-} -ffile-prefix-map=$ROOT_DIR=/flint -ffile-prefix-map=$CARGO_DIR=/cargo"
# cargo guarda lo compilado según RUSTFLAGS; un directorio aparte evita
# recompilar todo el target/ de desarrollo cada vez que se arma un paquete.
export CARGO_TARGET_DIR="$ROOT_DIR/target/dist"
