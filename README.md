# New Tricks

**Teach your agents new tricks.** New Tricks (`tricks`) is the design-time workbench for agent skills. Find skills across fragmented indexes, try them against real agents, customize them while still receiving upstream improvements, author your own, and publish a repository that APM, `npx skills`, Claude plugin marketplaces, Copilot, Codex and Cursor can all install.

Projects keep committing `apm.yml` and installing with [APM](https://github.com/microsoft/apm). New Tricks is what you use *before* that — see [SPEC.md](SPEC.md) for the full design.

```
discover ─► preview ─► install / link to test ─► vendor & customize ─► lint ─► publish
```

## Install

```bash
cargo install --path crates/newtricks        # or: brew install newtricks (tap), or a release binary
```

Requires `git`. Uses your existing GitHub credentials (`GITHUB_TOKEN`, then `gh auth token`); nothing is stored.

The VS Code extension (also Cursor, Windsurf, VSCodium) lives in [`extension/`](extension/).

## Quick start

```bash
# Discover: one search over skill repos, marketplaces, skills.sh and GitHub
tricks search pdf forms --license allow --no-scripts
tricks show anthropics/skills//skill-creator          # licence, risk, files, body

# Install for your agents (user scope, content-addressed store + links)
tricks add anthropics/skills//skill-creator --agents claude,codex
tricks status
tricks outdated && tricks update                 # review-first updates
tricks rollback skill-creator                         # instant, and pinned

# Let your agents drive New Tricks (pre-approves only read-only and branch-confined commands)
tricks agent-skill --agents claude,codex

# Try a skill inside a project without touching its git status
tricks link anthropics/skills//webapp-testing --to ~/code/my-app --agents claude
tricks unlink --all
```

### Author and customize

```bash
cd ~/code/my-skills && tricks init                    # any git repo becomes a workspace
tricks vendor anthropics/skills//skill-creator        # copy upstream in, record its base
tricks new changelog-writer --description "Writes release notes… Use when …"
tricks install                                        # dev-link workspace skills: edits are live

tricks edit changelog-writer --branch terse           # worktree experiment, agents load the draft
tricks commit changelog-writer -m "Terser output"
tricks use changelog-writer@terse [--local]           # pick the deployed variant

tricks update skill-creator                           # 3-way merge upstream changes into yours
tricks update --continue | --abort                    # after resolving conflicts
tricks diff skill-creator --from base --to working    # what did I change?
tricks lint [--fix] [--strict]                            # --strict: keys outside the spec are errors, like skills-ref
tricks pr skill-creator                               # send your change upstream
```

### Publish

```toml
# tricks.toml
[publish.targets.public]
repo    = "../my-skills-public"      # local checkout of the distribution repository
exclude = ["evals/**", "notes/**"]
```

```bash
tricks publish public --dry-run
tricks publish public --bump minor [--push | --pr]
```

The target gets `skills/<name>/`, a Claude `marketplace.json`, `apm.yml`, `PROVENANCE.md` and `CHANGELOG.md`, and is tagged `vX.Y.Z`. Gates: committed source, zero lint errors, licence policy for vendored skills, leak check, risk diff.

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
| `~/.config/newtricks/tricks.toml` | Workbench: settings, sources, installed skills, registered workspaces |
| `~/.config/newtricks/tricks.lock` | Resolved commits and tree hashes |
| `<workspace>/tricks.toml` / `.lock` | Workspace skills, upstreams, lint config, publish targets / recorded bases |
| `<workspace>/tricks.work.toml` | Machine-local variant overrides (gitignored) |

Data (store, mirrors, worktrees, `state.db`) lives in `~/Library/Application Support/newtricks` (macOS), `$XDG_DATA_HOME/newtricks` (Linux) or `%LOCALAPPDATA%\newtricks` (Windows).

Environment overrides: `TRICKS_HOME`, `TRICKS_CONFIG_DIR`, `TRICKS_DATA_DIR`, `TRICKS_GITHUB_TOKEN`, `TRICKS_LINK_MODE` (`copy`, `link`, or `agent=mode,…`).

## Development

```bash
cargo test                      # unit + hermetic integration tests (local "GitHub" fixtures)
cargo clippy --all-targets
cd extension && npm ci && npx tsc -p . && node out/test/protocol.test.js
```

Integration tests run the real binary against local repositories via `TRICKS_HOST_MAP="github.com=<dir>"`.

## Licence

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
