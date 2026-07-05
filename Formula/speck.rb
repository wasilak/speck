class Speck < Formula
  desc "Ultra-fast container runtime for Apple Silicon — VPN-proof networking"
  homepage "https://github.com/speck-vm/speck"
  version "0.1.0"
  license "AGPL-3.0-only"

  depends_on macos: ">= :ventura"

  on_arm do
    url "https://github.com/speck-vm/speck/releases/download/v#{version}/spk-#{version}-aarch64-apple-darwin.tar.gz"
    sha256 "<SHA256>"
  end

  def install
    bin.install "spk"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/spk --version")
  end
end
