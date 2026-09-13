#!/usr/bin/env bash
# Arma la fórmula de Homebrew para una versión, con los sha256 de los
# tar.gz que ya están construidos. Escribe en la salida estándar.
#
#     bash packaging/homebrew-formula.sh 0.2.0 dist > flint.rb
#
# La fórmula instala el binario ya compilado, no lo compila en la máquina de
# quien lo instala: para eso están los tar.gz de la release. Homebrew lo
# llama "binario embotellado a mano" y es lo normal para un proyecto que
# publica binarios.
set -euo pipefail

VERSION="${1:?falta la versión, por ejemplo 0.2.0}"
DIST="${2:-dist}"
REPO="nextsteptechnology2026-ai/flint"
BASE="https://github.com/$REPO/releases/download/v$VERSION"

sha() {
    local archivo="$DIST/flint-$VERSION-$1.tar.gz"
    if [ ! -f "$archivo" ]; then
        echo "falta $archivo" >&2
        exit 1
    fi
    # sha256sum en Linux, shasum en macOS.
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$archivo" | cut -d' ' -f1
    else
        shasum -a 256 "$archivo" | cut -d' ' -f1
    fi
}

SHA_MAC_ARM=$(sha aarch64-macos)
SHA_MAC_X86=$(sha x86_64-macos)
SHA_LINUX_X86=$(sha x86_64-linux)

cat <<RUBY
# Fórmula de Homebrew para Flint.
#
# Para publicarla, una sola vez: crear el repositorio
# ${REPO%%/*}/homebrew-flint (el prefijo "homebrew-" es obligatorio, brew lo
# da por sentado) y dejar este archivo en Formula/flint.rb. De ahí en más,
# cada release solo reemplaza ese archivo. Quien lo instala hace:
#
#     brew tap ${REPO%%/*}/flint
#     brew install flint
#
# Se genera sola en cada release (packaging/homebrew-formula.sh): los sha256
# salen de los tar.gz recién construidos, así que no hay forma de que queden
# apuntando a la versión anterior.
class Flint < Formula
  desc "Editor de terminal con capa modal opcional, LSP y multi-cursor"
  homepage "https://github.com/$REPO"
  version "$VERSION"
  license "MIT"

  on_macos do
    on_arm do
      url "$BASE/flint-$VERSION-aarch64-macos.tar.gz"
      sha256 "$SHA_MAC_ARM"
    end
    on_intel do
      url "$BASE/flint-$VERSION-x86_64-macos.tar.gz"
      sha256 "$SHA_MAC_X86"
    end
  end

  on_linux do
    on_intel do
      url "$BASE/flint-$VERSION-x86_64-linux.tar.gz"
      sha256 "$SHA_LINUX_X86"
    end
  end

  def install
    bin.install "flint"
    doc.install "README.md", "MANUAL.md", "theme.example.toml", "config.example.toml"
    # Los plugins de ejemplo van donde Flint los busca cuando está
    # instalado, igual que en el .deb.
    (share/"flint/plugins").install Dir["plugins/*"]
  end

  test do
    assert_match "$VERSION", shell_output("#{bin}/flint --version")
  end
end
RUBY
