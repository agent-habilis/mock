class Ahm < Formula
  desc "HTTP mock server for development and stable E2E tests"
  homepage "https://github.com/agent-habilis/mock"
  license "MIT"
  version "1.2.0"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/agent-habilis/mock/releases/download/v#{version}/ahm-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/agent-habilis/mock/releases/download/v#{version}/ahm-v#{version}-x86_64-unknown-linux-musl.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    elsif Hardware::CPU.arm?
      url "https://github.com/agent-habilis/mock/releases/download/v#{version}/ahm-v#{version}-aarch64-unknown-linux-musl.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "ahm"
    man1.install Dir["man/*.1"]
  end

  test do
    assert_match "ahm", shell_output("#{bin}/ahm --version")
  end
end
