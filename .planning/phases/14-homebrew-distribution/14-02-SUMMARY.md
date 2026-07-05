---
plan: 14-02
phase: 14-homebrew-distribution
status: complete
approach: Formula restored (deviates from plan — Cask/pkg path deferred)
---

# Plan 14-02 Summary — Homebrew Formula + README

## Deviation from plan

The plan described creating `Casks/spk.rb` and deleting `Formula/speck.rb`. Instead, the Formula was kept (improved) and no Cask was created, consistent with the ad-hoc signing decision in 14-01.

## What was actually done

**`Formula/speck.rb`** — improved in place:
- Updated `desc` to include "VPN-proof networking"
- Added `depends_on macos: ">= :ventura"` (Virtualization.framework requires it)
- Fixed URL: `spk-#{version}-aarch64-apple-darwin.tar.gz` (was `speck-#{version}`)
- Removed misleading comment about `--preserve-metadata=entitlements`
- Fixed test block: `assert_match version.to_s` (was broken string interpolation)

**`README.md`** — created (was absent entirely):
- Why section: the VPN/WARP problem and how Speck solves it
- Requirements: Apple Silicon, macOS 13+
- Build-from-source + ad-hoc signing instructions
- Homebrew install note (Formula, coming when releases exist)
- Quick start: `spk up`, `spk init`, `eval $(spk env)`, `spk doctor`
- Configuration: `~/.speck/config.yaml` with `vm` and `ca` blocks
- Feature status table: honest about what's done vs in-progress vs planned

## Deferred

`Casks/spk.rb` (pkg stanza, `depends_on macos: >= :ventura`, `uninstall pkgutil: io.speck.spk`) is documented in `14-02-PLAN.md` for the production distribution path.
