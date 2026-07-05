---
plan: 14-01
phase: 14-homebrew-distribution
status: complete
approach: ad-hoc signing (deviates from plan — Developer ID path deferred)
---

# Plan 14-01 Summary — Release Pipeline (Ad-hoc Signing)

## Deviation from plan

The plan described a full Developer ID Installer cert + pkgbuild + notarytool + stapler + Cask pipeline. That path was deferred pending Apple Developer Program membership. The release pipeline was simplified to ad-hoc signing for the dev/battle-testing phase.

## What was actually done

Rewrote `.github/workflows/release.yml` to a minimal pipeline:

- **Removed**: keychain management, Developer ID Application cert import, `--options runtime --timestamp` codesign flags, `ditto` zip prep, `xcrun notarytool`, `xcrun stapler`, `pkgbuild`, `softprops/action-gh-release@v2`, `actions/upload-artifact`
- **Added**: `codesign --sign -` with `speck.entitlements` (ad-hoc, embeds `com.apple.security.virtualization`), VERSION strip pattern (`${VERSION#v}`), `tar -czf spk-${VERSION}-aarch64-apple-darwin.tar.gz`, `gh release create` via automatic `GITHUB_TOKEN`

**Zero GitHub Secrets required.** `GITHUB_TOKEN` is injected automatically by Actions.

## Why ad-hoc works for now

Homebrew Formula re-signs with `--preserve-metadata=entitlements`, which preserves `com.apple.security.virtualization`. Homebrew also removes the quarantine xattr on install, bypassing Gatekeeper's notarization check. The entitlement is honored for locally-built and Homebrew-installed binaries.

## Deferred

Full Developer ID + `.pkg` + notarytool + Cask path is documented in `14-01-PLAN.md` for when the project reaches stable release.
