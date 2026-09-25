#!/usr/bin/env bash
# Acceptance scenario 8: a repository published by `tricks publish` installs through
# `npx skills`, Microsoft APM and Claude Code's plugin marketplace.
#
#   scripts/ecosystem-test.sh [path/to/tricks]
#
# Requires: git, node/npx, uv (for `uvx`), and the `claude` CLI. Everything runs in a
# temporary sandbox; no real agent or user configuration is touched.
set -euo pipefail

TRICKS="${1:-$(pwd)/target/release/tricks}"
TRICKS="$(cd "$(dirname "$TRICKS")" && pwd)/$(basename "$TRICKS")"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

export TRICKS_HOME="$T/home" TRICKS_CONFIG_DIR="$T/config" TRICKS_DATA_DIR="$T/data" TRICKS_NO_GH=1
export GIT_AUTHOR_NAME=CI GIT_AUTHOR_EMAIL=ci@example.com GIT_COMMITTER_NAME=CI GIT_COMMITTER_EMAIL=ci@example.com
mkdir -p "$TRICKS_HOME" "$TRICKS_CONFIG_DIR"
printf '[settings]\ndefault_catalogs = false\n' > "$TRICKS_CONFIG_DIR/tricks.toml"

pass() { printf '  \033[32m✓\033[0m %s\n' "$1"; }
fail() { printf '  \033[31m✗\033[0m %s\n' "$1"; exit 1; }

echo "== publish a source repo with tricks ($("$TRICKS" --version))"
# The distribution repository is a remote (bare, so it accepts pushes); TARGET is a
# clone of what was published, which the installers below read.
REMOTE="$T/demo-skills.git"
git init -q --bare -b main "$REMOTE"
git clone -q "$REMOTE" "$T/seed" 2>/dev/null
echo "# demo skills" > "$T/seed/README.md"
git -C "$T/seed" add -A && git -C "$T/seed" commit -qm init && git -C "$T/seed" push -q origin HEAD:main
TARGET="$T/demo-skills"

WS="$T/ws"
mkdir -p "$WS" && git -C "$WS" init -q -b main
(cd "$WS" && "$TRICKS" -q init --name demo >/dev/null)
(cd "$WS" && "$TRICKS" -q new greeter --description "Writes friendly greetings for any occasion. Use when the user asks for a greeting or welcome message." >/dev/null)
(cd "$WS" && "$TRICKS" -q new changelog-writer --description "Drafts release notes from git history. Use when the user asks for a changelog or release notes." >/dev/null)
for s in greeter changelog-writer; do
  printf -- '---\nname: %s\ndescription: %s\nlicense: MIT\n---\n\n# %s\n\n1. Read the request.\n2. Produce the output.\n' \
    "$s" "$(sed -n 's/^description: //p' "$WS/skills/$s/SKILL.md")" "$s" > "$WS/skills/$s/SKILL.md"
done
cat >> "$WS/tricks.toml" <<'EOF'

[publish.targets.public]
repo = "../demo-skills.git"
owner = "CI"
EOF
git -C "$WS" add -A && git -C "$WS" commit -qm "demo skills"
(cd "$WS" && "$TRICKS" -q publish public --bump minor --push --yes >/dev/null)
git clone -q "$REMOTE" "$TARGET"
[ -f "$TARGET/.claude-plugin/marketplace.json" ] && [ -f "$TARGET/apm.yml" ] && [ "$(git -C "$REMOTE" tag)" = "v0.1.0" ] \
  && pass "published v0.1.0 with marketplace.json and apm.yml" || fail "publish output incomplete"

echo "== npx skills"
LIST="$(HOME="$T/npx-home" npx -y skills@latest add "$TARGET" --list 2>&1 || true)"
echo "$LIST" | grep -q "greeter" && echo "$LIST" | grep -q "changelog-writer" \
  && pass "npx skills discovers both skills" || { echo "$LIST"; fail "npx skills did not list both skills"; }

echo "== Microsoft APM"
APM_PROJ="$T/apm-project"
mkdir -p "$APM_PROJ" && git -C "$APM_PROJ" init -q
cat > "$APM_PROJ/apm.yml" <<EOF
name: apm-consumer
version: 0.0.1
targets:
  - claude
dependencies:
  apm:
    - $TARGET/skills/greeter
EOF
(cd "$APM_PROJ" && HOME="$T/apm-home" uvx --python 3.12 --from apm-cli apm install >"$T/apm.log" 2>&1) || { cat "$T/apm.log"; fail "apm install failed"; }
[ -f "$APM_PROJ/.claude/skills/greeter/SKILL.md" ] && pass "apm install deployed greeter to .claude/skills" \
  || { cat "$T/apm.log"; fail "apm did not deploy the skill"; }

echo "== Claude Code plugin marketplace"
claude plugin validate "$TARGET" >"$T/validate.log" 2>&1 && grep -qi "passed" "$T/validate.log" \
  && pass "claude plugin validate" || { cat "$T/validate.log"; fail "claude plugin validate failed"; }
export CLAUDE_CONFIG_DIR="$T/claude-config"
mkdir -p "$CLAUDE_CONFIG_DIR"
HOME="$T/claude-home" claude plugin marketplace add "$TARGET" >"$T/mp.log" 2>&1 || { cat "$T/mp.log"; fail "marketplace add failed"; }
HOME="$T/claude-home" claude plugin install demo-skills@demo-skills >"$T/install.log" 2>&1 || { cat "$T/install.log"; fail "plugin install failed"; }
DETAILS="$(HOME="$T/claude-home" claude plugin details demo-skills@demo-skills 2>&1 || true)"
echo "$DETAILS" | grep -q "greeter" && echo "$DETAILS" | grep -q "changelog-writer" \
  && pass "claude plugin install loads both skills" || { echo "$DETAILS"; fail "installed plugin is missing skills"; }

echo "all ecosystem checks passed"
