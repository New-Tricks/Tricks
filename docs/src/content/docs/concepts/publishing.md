---
title: Publishing
description: Publish skills from your source repo to a distribution repository that APM, npx skills, Claude Code plugin marketplaces, Copilot, Codex and Cursor can install.
---

Your [source repo](/tricks/concepts/source-repo/) is where skills are made: drafts, experiment branches, vendoring bookkeeping, notes. None of that should reach the people who use your skills. [`tricks publish`](/tricks/reference/commands/publish/) copies the skills you choose into a separate **publish target**, a git repository laid out so every common installer understands it, after checking that what you ship is committed, lints clean and may legally be redistributed.

New Tricks stops there. Installing the published skills on a machine or in a project is the job of APM, `npx skills` and plugin marketplaces.

## Configure a target

Targets are defined in the source repo's `tricks.toml`:

```toml
[publish.targets.public]
repo    = "acme/my-skills-public"      # the distribution repository
exclude = ["evals/**", "notes/**"]     # never published
```

| Key | Default | Meaning |
|---|---|---|
| `repo` | (required) | The target's remote: `owner/repo`, `host/owner/repo`, any git URL, or a path to a repository |
| `skills` | `["*"]` | Skills to publish, by their `[skills.<name>]` key; `"*"` means all |
| `exclude` | `[]` | Globs, relative to each skill folder, of files to leave out |
| `plugins` | none | Optional table splitting skills into several marketplace plugins (see below) |
| `marketplace` | the target repository's name | Marketplace name in `marketplace.json` and `apm.yml`; must be kebab-case and not a name Claude Code reserves |
| `owner` | your git `user.name` | Marketplace owner name |
| `description` | `Agent skills published from <source repo>` | Marketplace and package description |

You can define several targets, for example a public one with a few skills and an internal one with all of them:

```toml
[publish.targets.internal]
repo   = "git@git.acme.internal:skills/internal.git"
skills = ["*"]
```

`repo` always names a remote, never a working copy. New Tricks publishes through its own clone of the target in its data directory and resets that clone to the remote's default branch before every run, so nothing left from an earlier run or a hand edit can leak into a release. A path is resolved against the source repo root and has to accept pushes, like a bare repository.

## Preview with `--dry-run`

```bash
tricks publish public --dry-run
```

```text
publish public → https://github.com/acme/my-skills-public.git (dry run)
  ✓ committed source
  ! lint
      NT206 skill-creator ~8232 tokens; the whole body loads on activation
      NT203 skill-creator `~/Downloads/eval_set.json` will not exist on other machines
  ✓ licence
      github.com/acme/my-skills-public is public
  ✓ leak check
  ! risk diff
      changelog-writer: new skill
      skill-creator: new skill
      skill-creator: + script scripts/check_links.py
      skill-creator: + script scripts/run_eval.py
      …
  version: none → untagged  (suggested bump: minor)
  changes:
    A .claude-plugin/marketplace.json
    A .tricks-published
    A CHANGELOG.md
    A PROVENANCE.md
    A apm.yml
    A skills/changelog-writer/SKILL.md
    A skills/skill-creator/LICENSE.txt
    A skills/skill-creator/SKILL.md
    …
```

A dry run runs every gate and computes every change without writing anything. It works on a dirty source repo (the first gate becomes a warning) and exits with status 1 when a gate fails, so you can run it in CI. `--json` gives the same report, including the changelog section, for tools.

## Gates

Gates run in this order. `✓` passes, `!` warns, `✗` blocks the publish.

1. **Committed source.** You can only publish a clean, committed source repo, so the published commit names an exact source commit. Commit or stash first.
2. **Lint.** Any [lint](/tricks/concepts/lint/) error in a published skill blocks; warnings are listed.
3. **Licence.** Vendored skills must allow redistribution to this target (see below). Your own skills are exempt.
4. **Leak check.** Likely secrets (API keys, tokens, private keys) block. Files that look private to the source repo, such as `.env`, `notes/`, `*.draft.*` and `.DS_Store`, are a warning: add them to `exclude`.
5. **Risk diff.** What got riskier since the last publish: new scripts, widened `allowed-tools`, new URLs, remote-execution patterns, hidden Unicode, removed skills. This never blocks, but a real publish asks you to confirm it.

### Licence gate

New Tricks detects each vendored skill's licence (a licence file in the skill, then frontmatter, the upstream repository root, the GitHub licence API, and a catalog's terms), takes the most restrictive answer, and applies a policy based on whether the target is public. Visibility is read from the GitHub API; if it can't be checked, the target is treated as public.

| Class | Examples | Public target | Private target |
|---|---|---|---|
| `allow` | MIT, MIT-0, Apache-2.0, BSD, ISC, Unlicense, CC0-1.0, CC-BY-4.0 | publish | publish |
| `weak-copyleft` | MPL-2.0, EPL-2.0, LGPL | publish with a warning | publish |
| `strong-copyleft` | GPL, AGPL, CC-BY-SA-4.0 | blocked unless `--accept-copyleft` | warning |
| `non-commercial` | CC-BY-NC-\*, PolyForm Noncommercial | blocked | warning |
| `block` | no licence, unknown, proprietary, no-derivatives, BUSL, SSPL | blocked | warning |

If you have permission to redistribute a blocked skill, for example a separate agreement with its authors, record it in `tricks.toml`. The blocked gate prints the exact snippet:

```toml
[skills.secret-sauce]
license-override = { justification = "Separate redistribution agreement with Acme" }
```

An override turns the block into a warning that names the licence, and shows up in every pre-flight. It is never applied automatically.

## What the target gets

```text
my-skills-public/
  skills/changelog-writer/SKILL.md
  skills/skill-creator/…                  # Agent Skills layout, with each skill's LICENSE and NOTICE files
  .claude-plugin/marketplace.json         # Claude Code plugin marketplace
  apm.yml                                 # APM package metadata
  PROVENANCE.md                           # where each skill came from
  CHANGELOG.md                            # grouped by skill
  LICENSE                                 # copied from your source repo root, if it has one
  .tricks-published                       # the paths New Tricks owns
```

- **`skills/<name>/`** holds each skill as it is in your source repo, minus `exclude` matches and leftover `.upstream` merge files. The only change to `SKILL.md` is in `metadata`: New Tricks-only `tricks-*` keys (such as `tricks-lint-disable`) are removed, and on a versioned publish `version` is set.
- **`.claude-plugin/marketplace.json`** lists one plugin containing every skill by default (`"source": "./"`, `"strict": false`), so Claude Code loads everything under `skills/` without a `plugin.json`.
- **`apm.yml`** carries metadata only (`name`, `version`, `description`, and `license` when your source repo has a root licence). APM installs skills without it; it gives APM users a package name and version.
- **`PROVENANCE.md`** is a table of each skill's upstream, base commit and licence, and the source commit published. The source repo is named by its `origin` remote with any credentials removed, or `local:<folder>` when it has none; a local path never appears, here or in the `Tricks-Source:` commit trailer.
- **`CHANGELOG.md`** gets a new section per publish, built from your source repo's commit subjects since the last publish, grouped by skill.

To split skills into several plugins, add groups. Skills not in any group stay in the default plugin:

```toml
[publish.targets.public.plugins]
documents = ["pdf", "docx"]
devops    = ["deploy-aws"]
```

### Hand-added files are safe

New Tricks owns exactly the paths listed in `.tricks-published`. A re-publish replaces those, including removing a skill you stopped publishing, and never touches anything else. Add a README, CI workflow or issue templates to the target by hand and they stay.

## Publish for real

A publish has to land somewhere visible, so it needs one of two flags:

```bash
tricks publish public --bump minor --push   # commit, tag v0.1.0 and push the default branch
tricks publish public --bump minor --pr     # push a branch and open a pull request with gh
```

```text
  version: none → 0.1.0  (suggested bump: minor)
  …
committed d6e158398, tagged v0.1.0
pushed
```

- `--push` commits to the target's default branch, tags it, and pushes both.
- `--pr` commits on a `tricks/publish-<version>` branch, pushes it and opens a pull request for review. It doesn't tag; tag the target after merging.
- `--yes` answers the risk-diff confirmation, for CI. Without a terminal and without `--yes`, a publish with a risk diff stops and asks you to re-run with `--yes`.

Every publish commit carries a trailer naming the source commit, for example `Tricks-Source: github.com/acme/my-skills@4e1f9a2…`.

## Versions

One version covers everything a source repo publishes; if you need separate version lines, use separate source repos.

- `--bump major|minor|patch` increments the latest `vX.Y.Z` tag on the target (starting from 0.0.0); `--bump 2.1.0` sets an exact version, which must be higher.
- The version is written to every published `SKILL.md` as `metadata.version`, and into `marketplace.json`, `apm.yml` and the changelog heading.
- Without `--bump` the publish is untagged: `SKILL.md` gets no version, `apm.yml` keeps the previous one (or `0.0.0`), and the changelog section is headed `Unreleased`.

The pre-flight suggests a bump, the largest across changed skills: **major** when a name changes, a description is rewritten, files are removed, `allowed-tools` widens or a skill is dropped; **minor** for new files or sections and new skills; **patch** for prose changes. It's a suggestion: you choose with `--bump`.

## Install what you published

The same repository works with every installer:

| Installer | Command |
|---|---|
| `npx skills` (Copilot, Codex, Cursor, Claude Code and more) | `npx skills add acme/my-skills-public` |
| APM | `apm install acme/my-skills-public/skills/changelog-writer`, or list it under `dependencies.apm` in a project's `apm.yml` |
| Claude Code | `/plugin marketplace add acme/my-skills-public`, then install the plugin from it |
| Anything else | copy `skills/<name>/` into the agent's skill directory |

APM needs each skill's `name` to match its folder and ASCII-only frontmatter; lint rules NT103 and NT403 catch both before you publish.
