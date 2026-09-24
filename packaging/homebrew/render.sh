#!/usr/bin/env bash
# Render the Homebrew formula for a release: insert the tagged source tarball's
# `url` and `sha256` after `homepage`.
#   packaging/homebrew/render.sh v0.1.0 > Formula/newtricks.rb
set -euo pipefail
tag="${1:?usage: render.sh <tag>}"
repo="${GITHUB_REPOSITORY:-new-tricks/tricks}"
url="https://github.com/$repo/archive/refs/tags/$tag.tar.gz"
tarball="$(mktemp)"
trap 'rm -f "$tarball"' EXIT
curl -fsSL --retry 3 "$url" -o "$tarball"
if command -v sha256sum >/dev/null; then sha="$(sha256sum "$tarball" | cut -d' ' -f1)"; else sha="$(shasum -a 256 "$tarball" | cut -d' ' -f1)"; fi
awk -v url="$url" -v sha="$sha" '
  { print }
  /^  homepage / { print "  url \"" url "\""; print "  sha256 \"" sha "\"" }
' "$(dirname "$0")/newtricks.rb"
