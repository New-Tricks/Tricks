# Packaging and release

## Channels

| Channel | How |
|---|---|
| GitHub Releases | Tag `vX.Y.Z` → `.github/workflows/release.yml` builds `tricks-<target>.tar.gz` + `.sha256` for macOS (arm64, x64), Linux (x64, arm64) and Windows (x64, arm64). `tricks self-update` consumes these. |
| Homebrew | Tap [`new-tricks/homebrew-tap`](https://github.com/new-tricks/homebrew-tap), formula `Formula/newtricks.rb` (installs the `tricks` binary; source copy in `packaging/homebrew/`). `brew install new-tricks/tap/newtricks`. The release workflow renders `packaging/homebrew/newtricks.rb` with the tag's source tarball `url`/`sha256` (`render.sh`), runs `brew style`, and pushes it to the tap using the `HOMEBREW_TAP_DEPLOY_KEY` secret (a write deploy key on the tap). Edit the template here, never the tap directly. Move to homebrew-core later. `self-update` defers to `brew upgrade`. |
| VS Code Marketplace | Platform-specific VSIX per target, each bundling its binary in `extension/bin/`. Needs the `VSCE_PAT` secret and a registered publisher (the placeholder `publisher` in `extension/package.json` is `newtricks`). Without the secret the step is skipped; upload the VSIX files by hand at marketplace.visualstudio.com/manage. Temporary — see [Later](#later). |
| Open VSX (Cursor, Windsurf, VSCodium) | Same VSIX files; needs the `OVSX_PAT` secret. |

## Before the first release

1. ~~GitHub org and repositories~~ — done: `new-tricks/tricks` and `new-tricks/homebrew-tap` (lowercase, matching the CLI, canonical skill IDs and GHCR/npm naming). The tap's formula is updated by the release workflow.
2. Register a VS Code Marketplace publisher and an Open VSX namespace; set `publisher`.
3. macOS signing and notarization: add an Apple Developer ID certificate (`APPLE_CERT_P12`, `APPLE_CERT_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD`) and replace the placeholder step with `codesign --options runtime` + `xcrun notarytool submit --wait`. Unsigned CLI binaries work when installed via Homebrew or from a tarball after `xattr -d com.apple.quarantine`.
4. Windows: optionally sign with Azure Trusted Signing.

## Re-running a release step

Actions → Release → *Run workflow* with an existing tag re-runs marketplace publishing (already published versions are skipped) and the Homebrew update, without rebuilding.

## Later

- **Switch VS Code Marketplace publishing to a Microsoft Entra identity, with no secret stored.** Publish from GitHub Actions with `vsce publish --azure-credential`, authenticated by `azure/login` over OIDC (a federated credential on an Entra app registration or user-assigned managed identity, trusted for this repository's release workflow and added as a member of the `newtricks` publisher). Then delete the `VSCE_PAT` secret. Reasons: Azure DevOps is retiring PATs scoped to all accessible organizations, which Marketplace publishing requires; a stored PAT expires within a year and is a long-lived credential. Blocker: it needs an Entra directory (tenant) — a personal Microsoft account has none by default, and signing in to the Azure Portal with one fails with `AADSTS16000`.

## Local builds

```bash
cargo build --release                      # target/release/tricks
cd extension && npm ci && npx tsc -p . && npx vsce package --no-dependencies
```

To test a platform VSIX locally, copy the release binary to `extension/bin/tricks` before packaging.
