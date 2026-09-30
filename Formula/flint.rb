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
  version "0.6.3"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.3/flint-0.6.3-aarch64-macos.tar.gz"
      sha256 "ae8583bfe84e71749b9f25b1c04fc85139c3536cf3154fcb51b0dae2704c6fe5"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.3/flint-0.6.3-x86_64-macos.tar.gz"
      sha256 "8faca7bf906e83352b559bd50e418749b5fa619274fb4de77cf85a9cfd8f12a6"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.3/flint-0.6.3-aarch64-linux.tar.gz"
      sha256 "1729eb71f5714c3a57752767d2a59c1d01f42be816ae2d7d7155cae8d4368be3"
    end
    on_intel do
      url "https://github.com/nextsteptechnology2026-ai/flint/releases/download/v0.6.3/flint-0.6.3-x86_64-linux.tar.gz"
      sha256 "7e1fabbf9b6dd0341446eb0284bb727bc51df072a445a2a590d6208e9b8912a1"
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
    assert_match "0.6.3", shell_output("#{bin}/flint --version")
  end
end
