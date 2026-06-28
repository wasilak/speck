class Speck < Formula
  desc "Ultra-fast container runtime for Apple Silicon"
  homepage "https://github.com/speck-vm/speck"
  version "0.1.0"
  license "AGPL-3.0-only"

  on_arm do
    url "https://github.com/speck-vm/speck/releases/download/v#{version}/speck-#{version}-aarch64-apple-darwin.tar.gz"
    sha256 "<SHA256>"
  end

  # Homebrew re-signs with --preserve-metadata=entitlements on install;
  # the virtualization entitlement is preserved automatically.
  def install
    bin.install "spk"
  end

  test do
    assert_match "spk #{version}", shell_output("#{bin}/spk --version")
  end
end
