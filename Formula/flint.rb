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
  version "0.6.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.0/flint-0.6.0-aarch64-macos.tar.gz"
      sha256 "3e30eaa4b4f5074bd7bf8f17388484ca7338bfa1abe86d6f3875c77cd8c3f373"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.0/flint-0.6.0-x86_64-macos.tar.gz"
      sha256 "76e66d0c6a1e62a87a14bc2506b0d0a2792a37679aaa4c3086a8ab79136db7b5"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.0/flint-0.6.0-aarch64-linux.tar.gz"
      sha256 "063b4071b64c3a7452e48c473dedf3668580569fd0ebc3fe0b1fafe7bb6ec5ab"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.0/flint-0.6.0-x86_64-linux.tar.gz"
      sha256 "871f6cc0f8cbbab2e565e5129bc4ea8363e688d426111696b8026cbd48226cd1"
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
    assert_match "0.6.0", shell_output("#{bin}/flint --version")
  end
end
