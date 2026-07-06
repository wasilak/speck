class Speck < Formula
  desc "Ultra-fast container runtime for Apple Silicon — VPN-proof networking"
  homepage "https://github.com/wasilak/speck"
  version "0.1.0"
  license "AGPL-3.0-only"

  depends_on macos: ">= :ventura"
  depends_on arch: :arm64

  on_arm do
    url "https://github.com/wasilak/speck/releases/download/v#{version}/spk-#{version}-aarch64-apple-darwin.tar.gz"
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end

  def install
    bin.install "spk"
  end

  def caveats
    <<~EOS
      First-run note: macOS may quarantine the binary (ad-hoc signing). To clear:
        xattr -dr com.apple.quarantine #{HOMEBREW_PREFIX}/bin/spk

      Set up shell integration (once):
        spk init --set-docker-host

      Start the VM:
        spk up
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/spk --version")
  end
end
