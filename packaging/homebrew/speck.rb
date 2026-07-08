cask "speck" do
  version "0.1.0"
  sha256 :no_check

  url "https://github.com/wasilak/speck/releases/download/v#{version}/spk-#{version}-aarch64-apple-darwin-development-non-notarized.tar.gz"
  name "Speck"
  desc "Ultra-fast container runtime for Apple Silicon — VPN-proof networking"
  homepage "https://github.com/wasilak/speck"

  depends_on macos: ">= :ventura"
  depends_on arch: :arm64

  pkg "spk-development-non-notarized.pkg"

  uninstall pkgutil: "io.speck.spk.dev"

  caveats <<~EOS
    This cask is a development-only install route for ad-hoc signed,
    non-notarized Speck artifacts. It is not an official Homebrew Cask and
    does not imply Developer ID signing, notarization, stapling, or
    Gatekeeper approval.

    Set up shell integration (once):
      spk init --set-docker-host

    Start the VM:
      spk up
  EOS
end
