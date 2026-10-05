# rendered by packaging/render.sh for each release, then pushed to the homebrew-mog tap
class Mog < Formula
  desc "Modeless terminal code editor that mogs other editors"
  homepage "https://github.com/SpideyZac/mog"
  version "{{version}}"
  license "Apache-2.0"

  on_macos do
    on_arm do
      url "https://github.com/SpideyZac/mog/releases/download/v{{version}}/mog-v{{version}}-aarch64-apple-darwin.tar.gz"
      sha256 "{{aarch64-apple-darwin}}"
    end
    on_intel do
      url "https://github.com/SpideyZac/mog/releases/download/v{{version}}/mog-v{{version}}-x86_64-apple-darwin.tar.gz"
      sha256 "{{x86_64-apple-darwin}}"
    end
  end

  on_linux do
    depends_on "alsa-lib"

    on_arm do
      url "https://github.com/SpideyZac/mog/releases/download/v{{version}}/mog-v{{version}}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "{{aarch64-unknown-linux-gnu}}"
    end
    on_intel do
      url "https://github.com/SpideyZac/mog/releases/download/v{{version}}/mog-v{{version}}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "{{x86_64-unknown-linux-gnu}}"
    end
  end

  def install
    bin.install "mog"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/mog --version")
  end
end
