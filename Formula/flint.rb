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
  version "0.6.1"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.1/flint-0.6.1-aarch64-macos.tar.gz"
      sha256 "7c967cad8ccf9bc4319f2e4640a552734feb206f46fc2934e06cf4fb837dafbc"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.1/flint-0.6.1-x86_64-macos.tar.gz"
      sha256 "749ab67534cf5ae773e3fb279712c36f97743b50925bc0f2dd7d06a9ddcd1672"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.1/flint-0.6.1-aarch64-linux.tar.gz"
      sha256 "3251f2e989d7e4798d6640c82b866a8f76787556f9cb49dba8b95d61782b6a30"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.1/flint-0.6.1-x86_64-linux.tar.gz"
      sha256 "9cc6bd438e9c292d0e096a3d641a68f04ec0210564de6633138c83616943b441"
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
    assert_match "0.6.1", shell_output("#{bin}/flint --version")
  end
end
