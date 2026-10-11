class Speck < Formula
  desc "Container runtime for Apple Silicon with VPN-proof networking"
  homepage "https://github.com/wasilak/speck"
  license "AGPL-3.0-only"

  depends_on arch: :arm64
  depends_on :macos

  on_macos do
    on_arm do
      url "https://github.com/wasilak/speck/releases/download/v1.4.1/spk-v1.4.1-aarch64-apple-darwin.tar.gz"
      sha256 "371c9fc4ae99f9b3ed638c7bc8069eb3dd81659004686cb8daf8a7badf441871"
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
