# New Tricks

**Teach your agents new tricks.** New Tricks (`tricks`) is the design-time workbench for agent skills. You work in a **source repo** — a git repository holding your own skills and customized copies of upstream skills — and New Tricks helps you find prior art across fragmented catalogs, keep vendored skills merging upstream improvements, try drafts and variants with real agents, lint them, and publish a repository that APM, `npx skills`, Claude plugin marketplaces, Copilot, Codex and Cursor can all install.

New Tricks does not manage the skills installed on your machine: that is what [APM](https://github.com/microsoft/apm), `npx skills` and plugin marketplaces are for, and what your published repository feeds. See [SPEC.md](SPEC.md) for the full design.

```
discover ─► create / vendor ─► edit on a branch ─► link & try with agents ─► merge ─► lint ─► publish
```

## Install

```bash
brew install new-tricks/tap/newtricks
cargo install --path crates/newtricks          # from a checkout
```

Requires `git`. Uses your existing GitHub credentials (`GITHUB_TOKEN`, then `gh auth token`); nothing is stored.

The VS Code extension (also Cursor, Windsurf, VSCodium) lives in [`extension/`](extension/).

## Quick start

```bash
# Discover prior art: one search over skill repos, marketplaces, skills.sh, Tessl, ClawHub and GitHub
tricks search pdf forms --license allow --no-scripts
tricks info anthropics/skills//skill-creator          # licence, risk, catalog signals, frontmatter, files
tricks view anthropics/skills//skill-creator | glow - # the content (pipe it to render)
tricks try anthropics/skills//webapp-testing          # try it in this project (git status stays clean)
tricks list --trials                                  # trials here (--all: everywhere)
tricks untry webapp-testing

# Start a source repo: any git repository
cd ~/code/my-skills && tricks init [--agent-skill]    # --agent-skill: teach this repo's agents New Tricks
tricks create changelog-writer --description "Writes release notes… Use when …"
tricks create my-skill --from ~/old/my-skill          # or take an existing folder
tricks vendor anthropics/skills//skill-creator        # copy an upstream skill in, record its base
tricks vendor clawhub.ai/awspace/skills//pdf          # catalog-hosted skills too (SHA-256 verified per file)
tricks list                                           # skills, variants, upstream changes

# Try your skills with real agents
tricks link                                           # every skill, user-level agent dirs, dev mode: edits are live
tricks link changelog-writer --to ~/code/my-app       # or one skill into one project
tricks list --links                                   # this repo's links (--all: every source repo's)
tricks unlink                                         # remove them

# Experiment on a branch
tricks edit changelog-writer -b terse --shell         # draft in .tricks/work/terse; linked agents load it
cd "$(tricks edit changelog-writer)"                  # ...or cd there (default branch: draft/<skill>)
tricks edit changelog-writer --commit -m "Terser output"
tricks diff changelog-writer head..terse
tricks use changelog-writer@terse [--local]           # pick the variant links deploy
tricks merge changelog-writer@terse [--pr] [--whole-branch]   # bring it back (only the skill, by default)

# Keep up with upstream
tricks outdated [--diff]                              # what changed upstream, with a risk summary
tricks sync skill-creator [--dry-run]                 # 3-way merge into yours, left uncommitted
tricks sync --continue | --abort                      # after resolving conflicts
tricks diff skill-creator base..                      # what did I change?
tricks contribute skill-creator                       # offer your change upstream as a pull request

tricks lint [--fix] [--strict]                        # --strict: keys outside the spec are errors, like skills-ref
tricks remove my-skill
```

To have the bundled `new-tricks` agent skill everywhere, install it like any published skill: `npx skills add new-tricks/tricks`.

### Publish

```toml
# tricks.toml
[publish.targets.public]
repo    = "acme/my-skills-public"    # distribution repository: owner/repo, a git URL or a path
exclude = ["evals/**", "notes/**"]
```

```bash
tricks publish public --dry-run
tricks publish public --bump minor --push   # or --pr for a reviewed pull request
```

The target gets `skills/<name>/`, a Claude `marketplace.json`, `apm.yml`, `PROVENANCE.md` and `CHANGELOG.md`, and is tagged `vX.Y.Z`. Gates: committed source, zero lint errors, licence policy for vendored skills, leak check, risk diff. A vendored skill whose licence blocks publishing can be allowed with a written reason: `[skills.<name>] license-override = { justification = "…" }`.

## Skill references

```
[host/]owner/repo//path-or-name[@ref]
anthropics/skills//pdf                          → github.com/anthropics/skills//skills/pdf@<latest tag or default branch>
github.mit.edu/ist-org/skills//docx@v1.0.0
https://github.com/anthropics/skills/tree/main/skills/pdf
```

`//` separates the repository from the in-repo path (it is required). `@ref` is Go style: tag, branch, commit, or `latest`.

## Configuration

| File | Purpose |
|---|---|
| `~/.config/newtricks/tricks.toml` | User config: settings, catalogs (the recommended ones are written on first run), registered source repos |
| `<source-repo>/tricks.toml` / `.lock` | Source repo skills, upstreams, lint config, publish targets / recorded bases |
| `<source-repo>/tricks.work.toml` | Machine-local variant overrides (gitignored) |

Data (store cache, mirrors, worktrees, `state.db`) lives in `~/Library/Application Support/newtricks` (macOS), `$XDG_DATA_HOME/newtricks` (Linux) or `%LOCALAPPDATA%\newtricks` (Windows).

Environment overrides: `TRICKS_HOME`, `TRICKS_CONFIG_DIR`, `TRICKS_DATA_DIR`, `TRICKS_GITHUB_TOKEN`, `TRICKS_LINK_MODE` (`copy`, `link`, or `agent=mode,…`).

## Development

```bash
cargo test                      # unit + hermetic integration tests (local "GitHub" fixtures)
cargo clippy --all-targets
cd extension && npm ci && npx tsc -p . && npm test   # protocol + webview (jsdom) tests
```

Integration tests run the real binary against local repositories via `TRICKS_HOST_MAP="github.com=<dir>"`.

## Licence

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
