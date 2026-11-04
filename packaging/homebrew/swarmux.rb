# Homebrew formula for a tap (a repository named homebrew-swarmux holding
# Formula/swarmux.rb). Fill in the placeholders for a release; see
# RELEASING.md. Users then run: brew install amirhosseinghanipour/swarmux/swarmux
class Swarmux < Formula
  desc "Operations center for one-person, agent-run companies, built on Claude Code"
  homepage "https://github.com/amirhosseinghanipour/swarmux"
  version "<VERSION>"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/amirhosseinghanipour/swarmux/releases/download/v#{version}/swarmux-aarch64-apple-darwin.tar.gz"
      sha256 "<SHA256 OF THE aarch64-apple-darwin ARCHIVE>"
    end
    on_intel do
      url "https://github.com/amirhosseinghanipour/swarmux/releases/download/v#{version}/swarmux-x86_64-apple-darwin.tar.gz"
      sha256 "<SHA256 OF THE x86_64-apple-darwin ARCHIVE>"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/amirhosseinghanipour/swarmux/releases/download/v#{version}/swarmux-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "<SHA256 OF THE x86_64-unknown-linux-gnu ARCHIVE>"
    end
  end

  def install
    bin.install "swarmux"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/swarmux --version")
  end
end
