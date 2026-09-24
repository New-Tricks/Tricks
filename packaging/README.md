# Packaging and release

## Channels

| Channel | How |
|---|---|
| GitHub Releases | Tag `vX.Y.Z` → `.github/workflows/release.yml` builds `tricks-<target>.tar.gz` + `.sha256` for macOS (arm64, x64), Linux (x64, arm64) and Windows (x64, arm64). `tricks self-update` consumes these. |
| Homebrew | `packaging/homebrew/newtricks.rb` (installs the `tricks` binary): start in a tap (`brew tap New-Tricks/tap`), move to homebrew-core later. `self-update` defers to `brew upgrade`. |
| VS Code Marketplace | Platform-specific VSIX per target, each bundling its binary in `extension/bin/`. Needs the `VSCE_PAT` secret and a registered publisher (the placeholder `publisher` in `extension/package.json` is `newtricks`). |
| Open VSX (Cursor, Windsurf, VSCodium) | Same VSIX files; needs the `OVSX_PAT` secret. |

## Before the first release

1. Register the GitHub org `new-tricks` (free as of September 2026) and create `New-Tricks/Tricks`; if you use a different owner, update `RELEASE_REPO` in `crates/newtricks/src/selfupdate.rs`, the repository URLs in `Cargo.toml`, `extension/package.json` and the formula.
2. Register a VS Code Marketplace publisher and an Open VSX namespace; set `publisher`.
3. macOS signing and notarization: add an Apple Developer ID certificate (`APPLE_CERT_P12`, `APPLE_CERT_PASSWORD`, `APPLE_TEAM_ID`, `APPLE_ID`, `APPLE_APP_PASSWORD`) and replace the placeholder step with `codesign --options runtime` + `xcrun notarytool submit --wait`. Unsigned CLI binaries work when installed via Homebrew or from a tarball after `xattr -d com.apple.quarantine`.
4. Windows: optionally sign with Azure Trusted Signing.

## Local builds

```bash
cargo build --release                      # target/release/tricks
cd extension && npm ci && npx tsc -p . && npx vsce package --no-dependencies
```

To test a platform VSIX locally, copy the release binary to `extension/bin/tricks` before packaging.
