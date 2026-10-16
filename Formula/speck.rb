class Speck < Formula
  desc "Container runtime for Apple Silicon with VPN-proof networking"
  homepage "https://github.com/wasilak/speck"
  url "https://github.com/wasilak/speck/releases/download/v1.4.5/spk-v1.4.5-aarch64-apple-darwin.tar.gz"
  sha256 "a55e12fa1a3829cd1262db4e74515357e20718f1e4b967abf571e74d3eabcbe1"
  license "AGPL-3.0-only"

  depends_on arch: :arm64
  depends_on :macos

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
