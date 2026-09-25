---
title: Quick start
description: A ten-minute walk-through from searching for a skill to testing your own skill and a branch experiment with real agents.
---

This walk-through takes you through the core loop with a small example: a source repo `~/code/my-skills` holding your own `changelog-writer` skill and a vendored copy of Anthropic's `skill-creator`, tested in a project `~/code/my-app`. The output shown is real, lightly trimmed; `tricks` prints full paths where this page shows `~`.

You need `tricks` [installed](/tricks/getting-started/installation/) and, ideally, GitHub credentials (`gh auth login`). Agents default to Claude Code; see [Agents](/tricks/concepts/agents/) to add Codex, Cursor or Copilot.

## 1. Find and try a skill

Search every catalog at once. The first search indexes the recommended catalogs, so it takes a little longer:

```bash
tricks search changelog
```

```text
docs-changelog  google-gemini/gemini-cli//.gemini/skills/docs-changelog
    Generates and formats changelog files for a new release based on provided version and raw changelog data.
    [official · licence:allow · 1.3k installs · ★107.2k]  in 1 catalog(s) · network references
changelog-automation  wshobson/agents//plugins/documentation-generation/skills/changelog-automation
    Automate changelog generation from commits, PRs, and releases following Keep a Changelog format. Use when setting up release workflows, generating release notes
    [unknown · licence:allow · 12.9k installs · ★40.0k · Tessl quality 77%]  in 2 catalog(s) · 1 variant(s) · network references
…
```

Each result shows its skill reference (`owner/repo//path`), trust, licence class, popularity and risk surface. Look closer at one before you use it:

```bash
tricks info anthropics/skills//skill-creator
```

```text
skill-creator  github.com/anthropics/skills//skills/skill-creator@main
  Create new skills, modify and improve existing skills, and measure skill performance. …
  commit   33375500b (default-branch main)
  trust    official
  licence  Apache-2.0 [allow] via skill-file
  risk     10 script(s); 7 URL reference(s)
  files:
    LICENSE.txt                                          11345
    SKILL.md                                             33168
    …
```

To see how an agent actually uses a skill, try it in a project. A trial goes into the current project's agent directories, and the project's `git status` stays clean:

```bash
cd ~/code/my-app
tricks try anthropics/skills//webapp-testing
```

```text
trying webapp-testing into ~/code/my-app
  claude   ~/code/my-app/.claude/skills/webapp-testing (link)
see trials with `tricks list --trials`; remove with `tricks untry webapp-testing`
```

Start your agent in `~/code/my-app` and it can use the skill. When you have seen enough, remove it:

```bash
tricks untry webapp-testing
```

More in [Discovery](/tricks/concepts/discovery/) and [Links and trials](/tricks/concepts/links-and-trials/).

## 2. Create a source repo

A source repo is any git repository. [`tricks init`](/tricks/reference/commands/init/) runs `git init` if needed, writes `tricks.toml` and `tricks.lock`, and registers the repo in your user config:

```bash
mkdir -p ~/code/my-skills && cd ~/code/my-skills
tricks init
```

```text
initialized a git repository in ~/code/my-skills
created source repo `my-skills` at ~/code/my-skills
next: `tricks create <name>` or `tricks vendor owner/repo//skill`, then `tricks link`
```

## 3. Create a skill

Scaffold your own skill with [`tricks create`](/tricks/reference/commands/create/):

```bash
tricks create changelog-writer \
  --description "Writes release notes from merged pull requests and commit history. Use when the user asks for a changelog, release notes or a summary of what shipped."
```

```text
created changelog-writer at skills/changelog-writer
  not committed yet; edit skills/changelog-writer/SKILL.md, then `tricks link changelog-writer` to try it
```

Open `skills/changelog-writer/SKILL.md` and write the instructions. For this walk-through, a numbered list of four steps under `## Instructions` is enough.

## 4. Vendor an upstream skill

[`tricks vendor`](/tricks/reference/commands/vendor/) copies an upstream skill into the repo so you can customize it, and records its upstream and base revision so you can merge upstream improvements later:

```bash
tricks vendor anthropics/skills//skill-creator
```

```text
added skill-creator at skills/skill-creator
  upstream github.com/anthropics/skills//skills/skill-creator @ 33375500b
  licence  Apache-2.0 [allow]
  risk     10 script(s); 7 URL reference(s)
  not committed yet: review and `git commit` when ready; `tricks link skill-creator` to try it
```

New Tricks never commits for you here. Review and commit with git, then look at the repo with [`tricks list`](/tricks/reference/commands/list/):

```bash
git add -A && git commit -m "Add changelog-writer, vendor skill-creator"
tricks list
```

```text
source repo my-skills (~/code/my-skills, branch main)
  changelog-writer       original
  skill-creator          from anthropics/skills//skills/skill-creator · lint 0E/2W
```

## 5. Link the skills to your agents

[`tricks link`](/tricks/reference/commands/link/) with no skill links every skill in the repo into the user-level agent directories, in dev mode: the agent reads your working tree, so edits are live.

```bash
tricks link
```

```text
linked changelog-writer into the user-level agent directories from main (working tree, live)
  claude   ~/.claude/skills/changelog-writer (link)
linked skill-creator into the user-level agent directories from main (working tree, live)
  claude   ~/.claude/skills/skill-creator (link)
see links with `tricks list --links`; remove them with `tricks unlink`
```

The agent directory now holds symlinks into your source repo:

```bash
ls -l ~/.claude/skills
```

```text
changelog-writer -> /Users/you/code/my-skills/skills/changelog-writer
skill-creator -> /Users/you/code/my-skills/skills/skill-creator
```

Start a new Claude Code session anywhere and ask for release notes: it loads `changelog-writer`.

## 6. Experiment on a branch

Try a terser style without touching the version your agents use now. [`tricks edit`](/tricks/reference/commands/edit/) checks the skill out on a branch, in a worktree inside the repo:

```bash
tricks edit changelog-writer -b terse
```

```text
~/code/my-skills/.tricks/work/terse/skills/changelog-writer
editing `changelog-writer` on branch terse (`cd "$(tricks edit changelog-writer)"` or `--shell` to work there); commit drafts with `tricks edit changelog-writer --commit -m "…"`, then `tricks merge changelog-writer@terse`
to try the draft with agents: `tricks link changelog-writer@terse`
```

Edit the draft's `SKILL.md` at that path. Then link the draft into one project only, so you can compare it with the version everywhere else:

```bash
tricks link changelog-writer@terse --to ~/code/my-app
tricks list
```

```text
linked changelog-writer into ~/code/my-app from terse (draft, live, pinned)
  claude   ~/code/my-app/.claude/skills/changelog-writer (link)
see links with `tricks list --links`; remove with `tricks unlink changelog-writer`
source repo my-skills (~/code/my-skills, branch main)
  changelog-writer       original · editing on terse · linked: ~/code/my-app (terse), user level (main)
  skill-creator          from anthropics/skills//skills/skill-creator · lint 0E/2W · linked: user level (main)
```

Agents working in `~/code/my-app` now load the draft; agents everywhere else still load `main`. When you like the result, commit the draft on its branch and compare:

```bash
tricks edit changelog-writer --commit -m "Terser changelog lines"
tricks diff changelog-writer head..terse
```

```text
committed changelog-writer 121c5691f on terse
--- head/changelog-writer/SKILL.md
+++ terse/changelog-writer/SKILL.md
@@ -10,7 +10,8 @@
 1. Find the last release tag with `git describe --tags --abbrev=0`.
 2. List merged pull requests and commits since that tag.
 3. Group changes under Added, Changed, Fixed and Removed.
-4. Write one line per change, in the past tense, naming the user-visible effect.
+4. Write one line per change, at most twelve words, naming the user-visible effect.
+5. Leave out internal refactors and dependency bumps.
```

More in [Branch experiments](/tricks/concepts/branch-experiments/).

## 7. Merge it back

[`tricks merge`](/tricks/reference/commands/merge/) brings only the skill's folder back from the branch, as one commit. Links pinned to the branch return to following `main`, which now has the change:

```bash
tricks merge changelog-writer@terse
```

```text
merged changelog-writer from terse into main (5c8ca90e3)
  → ~/code/my-app/.claude/skills/changelog-writer (link)
  → ~/.claude/skills/changelog-writer (link)
```

## 8. Lint

[`tricks lint`](/tricks/reference/commands/lint/) checks every skill in the repo:

```bash
tricks lint
```

```text
warning NT206 skill-creator/SKILL.md  ~8156 tokens; the whole body loads on activation
warning NT203 skill-creator/SKILL.md:371  `~/Downloads/eval_set.json` will not exist on other machines
0 error(s), 2 warning(s)
```

Warnings don't block anything; errors block publishing. Fixing the NT203 warning is your first customization of `skill-creator`: edit the path, commit, and `tricks list` marks the skill `customized`. See [Lint](/tricks/concepts/lint/) and the [rule table](/tricks/reference/lint-rules/).

## 9. Clean up and publish

Links are for testing. Remove this repo's links when you are done:

```bash
tricks unlink
```

```text
removed ~/code/my-app/.claude/skills/changelog-writer
removed ~/.claude/skills/changelog-writer
removed ~/.claude/skills/skill-creator
```

To share the skills, add a publish target to `tricks.toml` and preview a release:

```toml
[publish.targets.public]
repo = "acme/my-skills-public"
```

```bash
tricks publish public --dry-run
```

The published repository installs with `npx skills`, APM and Claude plugin marketplaces. See [Publishing](/tricks/concepts/publishing/).

## Next steps

- Keep `skill-creator` current as upstream changes: [Upstream tracking](/tricks/concepts/upstream/) and [Customize an upstream skill](/tricks/guides/customize-an-upstream-skill/).
- Everything `tricks list` can tell you: [Source repo](/tricks/concepts/source-repo/).
- Let your coding agent drive New Tricks: [Working with coding agents](/tricks/guides/working-with-coding-agents/).
