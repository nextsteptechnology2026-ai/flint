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
  version "0.3.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.3.0/flint-0.3.0-aarch64-macos.tar.gz"
      sha256 "9e1c0745564b991d9d3be60030516721d01fa6a6adea776d6727e63e87bf401f"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.3.0/flint-0.3.0-x86_64-macos.tar.gz"
      sha256 "ca137cfbd7b0838747f604bb523b241a6e963d1a9f5a7fc501d1d69cd89f4c9c"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.3.0/flint-0.3.0-x86_64-linux.tar.gz"
      sha256 "fc1c608c3799c241543711cd853321bff9057df982f2e79bed8c1c89cd7fce3e"
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
    assert_match "0.3.0", shell_output("#{bin}/flint --version")
  end
end
