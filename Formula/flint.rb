# Fórmula de Homebrew para Flint.
#
# Vive en Formula/flint.rb del mismo repositorio de Flint, así que el tap es
# el repositorio entero y no hace falta uno aparte "homebrew-flint". Como el
# nombre no empieza con "homebrew-", brew necesita la URL explícita:
#
#     brew tap nextsteptechnology2026-ai/flint https://github.com/nextsteptechnology2026-ai/flint
#     brew install nextsteptechnology2026-ai/flint/flint
#
# El nombre completo en el install es obligatorio: homebrew-core ya tiene una
# fórmula "flint" (la librería de teoría de números) y un "brew install flint"
# a secas instala esa.
#
# Se genera sola en cada release (packaging/homebrew-formula.sh) y el workflow
# la sube a main: los sha256 salen de los tar.gz recién construidos, así que
# no hay forma de que queden apuntando a la versión anterior.
class Flint < Formula
  desc "Editor de terminal con capa modal opcional, LSP y multi-cursor"
  homepage "https://github.com/nextsteptechnology2026-ai/flint"
  version "0.6.2"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.2/flint-0.6.2-aarch64-macos.tar.gz"
      sha256 "3331fbf32ba0393cd1a17b1d9e995a53541fc14afdab2b688c2337f6c07ab912"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.2/flint-0.6.2-x86_64-macos.tar.gz"
      sha256 "022fc3cc15553706cbe4b86a2a481d455dd458a9c1b46af7fe3d5e6e7dcc16dd"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.2/flint-0.6.2-aarch64-linux.tar.gz"
      sha256 "3d3da8b8064727bacff13ac9428a247de37e507a6128c97515ad7ee607a1b3bb"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.2/flint-0.6.2-x86_64-linux.tar.gz"
      sha256 "078ece8df4e03a2750f245356e4116edded54bfd993c17dec6933ff83b4253ea"
    end
  end

  def install
    bin.install "flint"
    doc.install "README.md", "MANUAL.md", "theme.example.toml", "config.example.toml"
    # Los plugins de ejemplo van al share/ del prefijo: Flint los busca ahí,
    # al lado de su propio binario (el .deb usa /usr/share, que es lo mismo
    # para /usr/bin/flint).
    (share/"flint/plugins").install Dir["plugins/*"]
  end

  test do
    assert_match "0.6.2", shell_output("#{bin}/flint --version")
  end
end
