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
  version "0.6.4"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.4/flint-0.6.4-aarch64-macos.tar.gz"
      sha256 "ea9591e90ba939c68befc3b99aa9e391d0bc75618229199cb52fd50f54e14eab"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.4/flint-0.6.4-x86_64-macos.tar.gz"
      sha256 "f4aa57ae50586e34ba2f164198e27f6ea9541e87bad14c02c54a5613840e61ce"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.4/flint-0.6.4-aarch64-linux.tar.gz"
      sha256 "d0a67211c809b6731fda516b63147d3afa86da5cc4ff3453a0101ea144864e53"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.4/flint-0.6.4-x86_64-linux.tar.gz"
      sha256 "20e33d99fc6e0e41ac70f8812b7af2d829b47ba0d03cec4ab9dbb01b55188220"
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
    assert_match "0.6.4", shell_output("#{bin}/flint --version")
  end
end
