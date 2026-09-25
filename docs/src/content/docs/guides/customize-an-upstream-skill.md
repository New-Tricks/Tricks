---
title: Customize an upstream skill
description: Find an upstream skill, check and try it, vendor it into your source repo, make it yours, and keep merging upstream improvements, including resolving a conflict.
---

This guide takes Anthropic's `skill-creator` skill, adapts it to your team's conventions, and keeps it up to date as Anthropic improves it. It assumes you have a source repo at `~/code/my-skills` (see [Quick start](/tricks/getting-started/quick-start/)) and a project `~/code/my-app` to test in.

## 1. Find it

```bash
tricks search skill creator --limit 4
```

```text
skill-creator  anthropics/skills//skills/skill-creator
    Create new skills, modify and improve existing skills, and measure skill performance. Use when users want to create a skill from scratch, edit, or optimize an e
    [official · licence:unknown · 390.9k installs · ★178.2k]  in 2 catalog(s) · 16 variant(s) · scripts
pr-creator  google-gemini/gemini-cli//.gemini/skills/pr-creator
    …
skill-creator  openai/skills//skills/.system/skill-creator
    Guide for creating effective skills. This skill should be used when users want to create a new skill (or update an existing skill) that extends Codex's capabili
    [official · licence:unknown · 4.3k installs · ★27.6k · Tessl quality 76%]  in 2 catalog(s) · 16 variant(s) · scripts
```

Two popular skills share the name, and 16 variants exist. The identifier after the name tells them apart. See [Discovery](/tricks/concepts/discovery/) for filters such as `--license allow` and `--no-scripts`.

## 2. Check it

```bash
tricks info anthropics/skills//skill-creator
```

```text
skill-creator  github.com/anthropics/skills//skills/skill-creator@main
  …
  trust    official
  licence  Apache-2.0 [allow] via skill-file
  risk     10 script(s); 7 URL reference(s)
  installs 390.9k
  listed   clawhub, skills.sh
  files:
    LICENSE.txt                                          11345
    SKILL.md                                             33168
    …
```

The full licence check finds Apache-2.0 in the skill's `LICENSE.txt` (search showed `unknown` because it only looks at frontmatter). The skill ships ten scripts; read the ones that matter before you let an agent run them:

```bash
tricks view anthropics/skills//skill-creator | glow -
tricks view anthropics/skills//skill-creator scripts/run_eval.py
```

## 3. Try it

Try the skill as-is in a project, with a real agent, before you decide to own a copy:

```bash
cd ~/code/my-app
tricks try anthropics/skills//skill-creator
```

```text
trying skill-creator into ~/code/my-app
  claude   ~/code/my-app/.claude/skills/skill-creator (link)
see trials with `tricks list --trials`; remove with `tricks untry skill-creator`
```

Start your agent in `~/code/my-app` and use it. `git status` in the project stays clean. When you're done:

```bash
tricks untry skill-creator
```

Remove the trial before you link your own copy: both would be called `skill-creator` in the same directory, and New Tricks refuses to overwrite one with the other. More in [Links and trials](/tricks/concepts/links-and-trials/).

## 4. Vendor it

```bash
cd ~/code/my-skills
tricks vendor anthropics/skills//skill-creator
git add -A && git commit -m "Vendor skill-creator"
```

```text
added skill-creator at skills/skill-creator
  upstream github.com/anthropics/skills//skills/skill-creator @ 079fa83de
  licence  Apache-2.0 [allow]
  risk     10 script(s); 7 URL reference(s)
  not committed yet: review and `git commit` when ready; `tricks link skill-creator` to try it
```

The skill is now in `skills/skill-creator/`, and `tricks.lock` records the upstream commit you started from. Commit the vendored copy unchanged first, so your customization is a separate commit you can read later.

## 5. Customize it

Edit the files like any other code. Here the opening line of `SKILL.md` points at a new house-style reference:

```bash
$EDITOR skills/skill-creator/SKILL.md
$EDITOR skills/skill-creator/references/house-style.md
tricks lint skill-creator
tricks link skill-creator --to ~/code/my-app          # test your version with an agent
git add -A && git commit -m "Point skill-creator at our house style"
```

For larger changes, experiment on a branch with `tricks edit skill-creator -b <branch>` and link the draft into one project; see [Branch experiments](/tricks/concepts/branch-experiments/).

Check what you changed relative to upstream at any time:

```bash
tricks diff skill-creator base..
```

```text
--- base/skill-creator/SKILL.md
+++ working/skill-creator/SKILL.md
@@ -27,7 +27,7 @@

 Then after the skill is done (but again, the order is flexible), you can also run the skill description improver, which we have a whole separate script for, to optimize the triggering of the skill.

-Cool? Cool.
+Before you start, read [our house style](references/house-style.md): it covers naming, tone and the review checklist our team uses.

 ## Communicating with the user
```

## 6. Later: see what upstream changed

Weeks later, `tricks list` shows `upstream has main` next to the skill (from cached state). Fetch and review:

```bash
tricks outdated --diff
```

```text
skill-creator            079fa83de → main
    M SKILL.md
    A scripts/check_links.py
    risk + script scripts/check_links.py
    risk +1 URL(s): https://agentskills.io/specification
--- base/skill-creator/SKILL.md
+++ upstream/skill-creator/SKILL.md
@@ -5,7 +5,7 @@
…
+++ upstream/skill-creator/scripts/check_links.py
@@ -0,0 +1,7 @@
+import sys, pathlib, re
…
```

Upstream added a script. Read it before you take it: once merged and linked, your agents can run it.

## 7. Preview and merge

```bash
tricks update skill-creator --dry-run     # what will change in your version
tricks update skill-creator
```

```text
merged       skill-creator → main
    incoming M SKILL.md
    incoming A scripts/check_links.py
    merged   SKILL.md
    updated  scripts/check_links.py
    risk     + script scripts/check_links.py
    risk     +1 URL(s): https://agentskills.io/specification
    merged into the working tree (uncommitted); review with `git diff` and commit — agents keep the previous version until then
```

Your house-style line and upstream's changes are both in the file. Review with `git diff`, then commit:

```bash
git diff
git add -A && git commit -m "Update skill-creator from upstream"
```

Until you commit, the link in `~/code/my-app` keeps serving your last committed version; after the commit, the next `tricks` command puts it back on your working tree.

## 8. Resolve a conflict

Next time, upstream rewrites the very line you changed. `update` stops with conflict markers:

```bash
tricks update skill-creator
```

```text
conflicts    skill-creator → main
    incoming M SKILL.md
    CONFLICT SKILL.md (text)
    resolve the conflicts, then run `tricks update --continue` (or `--abort`)
```

```text
<<<<<<< skill-creator (yours)
Before you start, read [our house style](references/house-style.md): it covers naming, tone and the review checklist our team uses.
=======
Ready? Start by asking the user what the skill should do.
>>>>>>> upstream main
```

Edit `SKILL.md` to what you want. Here, keep both lines:

```text
Ready? Start by asking the user what the skill should do.

Before you start, read [our house style](references/house-style.md): it covers naming, tone and the review checklist our team uses.
```

Then finish and commit:

```bash
tricks update --continue
git add -A && git commit -m "Merge upstream opening into skill-creator"
```

```text
continued    skill-creator → 98225f475
    update completed (uncommitted); review and commit
```

`--continue` refuses while conflict markers remain. To back out instead, `tricks update --abort` restores your version exactly as it was.

## 9. Confirm your customization survived

```bash
tricks diff skill-creator base..
```

```text
--- base/skill-creator/SKILL.md
+++ working/skill-creator/SKILL.md
@@ -29,6 +29,8 @@

 Ready? Start by asking the user what the skill should do.

+Before you start, read [our house style](references/house-style.md): it covers naming, tone and the review checklist our team uses.
+
 ## Communicating with the user
```

Your change is now expressed against the new base, and `tricks outdated` reports `all vendored skills are up to date`.

## 10. Optional: contribute it back

If the change would help everyone, offer it upstream. Preview first:

```bash
tricks contribute skill-creator --dry-run
```

```text
skill-creator → github.com/anthropics/skills (branch tricks/skill-creator-1790368223)
 skills/skill-creator/SKILL.md                  | 2 ++
 skills/skill-creator/references/house-style.md | 5 +++++
 2 files changed, 7 insertions(+)
```

Without `--dry-run`, New Tricks shows the files that will become public, asks you to confirm, forks the upstream repository with `gh`, and opens a pull request containing only this skill's change.

## Next

- Keep a vendored skill on its current version with `update = "pinned"`, or stop checking it with `paused`: see [update policies](/tricks/concepts/upstream/#update-policies).
- Ship it: [Publishing](/tricks/concepts/publishing/) carries the upstream licence and records provenance for vendored skills.
