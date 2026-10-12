class Speck < Formula
  desc "Container runtime for Apple Silicon with VPN-proof networking"
  homepage "https://github.com/wasilak/speck"
  license "AGPL-3.0-only"

  depends_on arch: :arm64
  depends_on :macos

  on_macos do
    on_arm do
      url "https://github.com/wasilak/speck/releases/download/v1.4.3/spk-v1.4.3-aarch64-apple-darwin.tar.gz"
      sha256 "2c8ab759dc347687041e6ce7c862e39a5f3a9684a04cb8b4dbd457eb1d988edb"
    end
  end

  def install
    bin.install "spk"
    generate_completions_from_executable(bin/"spk", "completion")
  end

  def caveats
    <<~EOS
      Start Speck (selects the 'speck' docker context, like colima start):
        spk start

      Stop / restart / remove the VM:
        spk stop
        spk restart
        spk delete

      The binary is ad-hoc signed with the virtualization entitlement and
      not notarized.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/spk --version")
  end
end
