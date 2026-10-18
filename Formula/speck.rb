class Speck < Formula
  desc "Container runtime for Apple Silicon with VPN-proof networking"
  homepage "https://github.com/wasilak/speck"
  url "https://github.com/wasilak/speck/releases/download/v1.4.8/spk-v1.4.8-aarch64-apple-darwin.tar.gz"
  sha256 "b334779b1217fc1c26c1aa3e4aa646da0a2cc2a9d50d01d3ca9c7a0071a49f00"
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
