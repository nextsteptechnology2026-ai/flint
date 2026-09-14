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
  version "0.2.0"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.2.0/flint-0.2.0-aarch64-macos.tar.gz"
      sha256 "a9800392ed7d69529aed00718b3c59e5e7105b2ec199ce66b9c76d2d26f948a9"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.2.0/flint-0.2.0-x86_64-macos.tar.gz"
      sha256 "a2b406ff4921ecff4b4248a25efd33efb5c5a5216cee0043b903020239c56e98"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.2.0/flint-0.2.0-x86_64-linux.tar.gz"
      sha256 "3cc6cc459d24bdc49908acae7e4f458288d2443454720eaf35a1da637affd191"
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
    assert_match "0.2.0", shell_output("#{bin}/flint --version")
  end
end
