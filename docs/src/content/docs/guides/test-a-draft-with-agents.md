---
title: Test a draft with agents
description: Compare the current version of a skill with a draft on a branch, side by side in real agents, then merge the draft or drop it.
---

In this guide you keep the current `changelog-writer` linked for every project, test a terser draft in one project, and then merge the draft or throw it away. At the end, you try an upstream skill in a project before deciding whether to vendor it.

You need a [source repo](/tricks/concepts/source-repo/) (here `~/code/my-skills`) with `changelog-writer` committed on `main`, and a project to test in (`~/code/my-app`). The examples use Claude Code, the default agent. See [Agents](/tricks/concepts/agents/) to link for others.

## 1. Link the current version everywhere

```bash
cd ~/code/my-skills
tricks link changelog-writer
```

```text
linked changelog-writer into the user-level agent directories from main (working tree, live)
  claude   ~/.claude/skills/changelog-writer (link)
see links with `tricks list --links`; remove with `tricks unlink changelog-writer`
```

The user-level link points at your working tree, so every project's agent loads the `main` version and sees your edits as soon as you save.

## 2. Start a draft on a branch

```bash
tricks edit changelog-writer -b terse
```

```text
~/code/my-skills/.tricks/work/terse/skills/changelog-writer
editing `changelog-writer` on branch terse (`cd "$(tricks edit changelog-writer)"` or `--shell` to work there); commit drafts with `tricks edit changelog-writer --commit -m "…"`, then `tricks merge changelog-writer@terse`
to try the draft with agents: `tricks link changelog-writer@terse`
```

The draft is a git worktree of branch `terse` at `.tricks/work/terse`, which git ignores. Open it in your editor, or `cd "$(tricks edit changelog-writer)"`. Editing it doesn't change what your agents load yet.

## 3. Link the draft into one project

```bash
tricks link changelog-writer@terse --to ~/code/my-app
tricks list --links
```

```text
linked changelog-writer into ~/code/my-app from terse (draft, live, pinned)
  claude   ~/code/my-app/.claude/skills/changelog-writer (link)
see links with `tricks list --links`; remove with `tricks unlink changelog-writer`
user level:
  changelog-writer             claude   ~/.claude/skills/changelog-writer (link)  main (working tree, live)
~/code/my-app:
  changelog-writer             claude   ~/code/my-app/.claude/skills/changelog-writer (link)  terse (draft, live, pinned)
```

The project link is pinned to `terse`, and the user-level link stays on `main`. The project link is added to `my-app`'s `.git/info/exclude`, so `git status` in `my-app` stays clean.

:::caution[Same name at two levels]
When a user-level skill and a project skill have the same name, Claude Code uses the user-level (personal) one. To make sure the agent in `my-app` loads the draft, remove the user-level link while you compare (`tricks unlink changelog-writer --global`), and compare against `main` in another project instead (`tricks link changelog-writer --to ~/code/other-app`). Run `tricks link changelog-writer` again when you're done.
:::

## 4. Try it and iterate

Open the agent in the project and ask for release notes:

```bash
cd ~/code/my-app && claude
```

Then edit the draft's `SKILL.md`. The link points into the worktree, so the agent in `my-app` gets what you saved. Start a new session if it has already loaded the skill. Other projects keep getting `main`. Compare the two by running the same prompt in `my-app` and in another project.

While you're iterating, you can check where each version comes from with `tricks list`:

```text
source repo my-skills (~/code/my-skills, branch main)
  changelog-writer       original · editing on terse · linked: ~/code/my-app (terse), user level (main)
  skill-creator          from anthropics/skills//skills/skill-creator · lint 0E/2W
```

## 5. Commit the draft

```bash
tricks edit changelog-writer --commit -m "Terser output"
```

```text
committed changelog-writer 195438adc on terse
```

This commits only the skill's folder, on `terse`. Run it as often as you like, like any commit.

## 6. Finish editing

```bash
tricks edit changelog-writer --done
tricks list --links
```

```text
~/code/my-skills/.tricks/work/terse/skills/changelog-writer
finished editing `changelog-writer`; links pinned to terse deploy its last commit
links on terse:
  → ~/code/my-app/.claude/skills/changelog-writer (link)
user level:
  changelog-writer             claude   ~/.claude/skills/changelog-writer (link)  main (working tree, live)
~/code/my-app:
  changelog-writer             claude   ~/code/my-app/.claude/skills/changelog-writer (link)  terse @ 195438a (snapshot, pinned)
```

The project keeps testing `terse`, now as a read-only snapshot of the branch tip. If you commit more to `terse`, the snapshot follows on the next `tricks` command you run in the source repo.

## 7. Compare the versions

```bash
tricks diff changelog-writer head..terse
```

```text
--- head/changelog-writer/SKILL.md
+++ terse/changelog-writer/SKILL.md
@@ -9,4 +9,4 @@
 
 1. List the pull requests merged since the last tag.
 2. Group them under Added, Changed, Fixed and Removed.
-3. Write one sentence per change, explaining what it means for users.
+3. One line per change, at most 12 words. No preamble.
```

## 8a. Keep it: merge

```bash
tricks merge changelog-writer@terse
tricks list --links
```

```text
merged changelog-writer from terse into main (73f85f733)
  → ~/code/my-app/.claude/skills/changelog-writer (link)
  → ~/.claude/skills/changelog-writer (link)
user level:
  changelog-writer             claude   ~/.claude/skills/changelog-writer (link)  main (working tree, live)
~/code/my-app:
  changelog-writer             claude   ~/code/my-app/.claude/skills/changelog-writer (link)  main (working tree, live)
```

`merge` committed only the skill's folder from `terse` onto `main`, and moved the pinned link back to `main`, which now has the change. To open a pull request on your source repo's remote instead, use `--pr`. [Merge it back](/tricks/concepts/branch-experiments/#merge-it-back) covers the options and conflicts.

The branch and its worktree are still there. Remove them when you're done. A skill-only merge is applied as a patch, so git needs `-D`:

```bash
git worktree remove .tricks/work/terse
git branch -D terse
```

## 8b. Drop it: unlink and delete the branch

If the draft isn't better, remove the project link and the branch. The user-level link never changed.

```bash
tricks edit changelog-writer --done               # if you're still editing
tricks unlink changelog-writer --to ~/code/my-app
git worktree remove .tricks/work/terse
git branch -D terse
```

```text
removed ~/code/my-app/.claude/skills/changelog-writer
```

`git branch -D` refuses while the worktree still has the branch checked out, so remove the worktree first.

## Test an upstream skill before vendoring it

Before you [vendor](/tricks/guides/customize-an-upstream-skill/) a skill someone else wrote, try it as is in a project. A trial is an exact revision from the store, and it's never updated:

```bash
cd ~/code/my-app
tricks try anthropics/skills//webapp-testing
tricks list --trials
```

```text
trying webapp-testing into ~/code/my-app
  claude   ~/code/my-app/.claude/skills/webapp-testing (link)
see trials with `tricks list --trials`; remove with `tricks untry webapp-testing`
~/code/my-app:
  anthropics/skills//skills/webapp-testing claude   ~/code/my-app/.claude/skills/webapp-testing (link)
```

Use it with the agent in `my-app`. Then remove the trial and, if you want to keep and customize the skill, vendor it into your source repo:

```bash
tricks untry webapp-testing
cd ~/code/my-skills
tricks vendor anthropics/skills//webapp-testing
```

```text
removed ~/code/my-app/.claude/skills/webapp-testing
added webapp-testing at skills/webapp-testing
  upstream github.com/anthropics/skills//skills/webapp-testing @ 33375500b
  licence  Apache-2.0 [allow]
  risk     4 script(s); 3 URL reference(s)
  not committed yet: review and `git commit` when ready; `tricks link webapp-testing` to try it
```

From now on it's a source repo skill: you `link` it, experiment on branches as above, and [merge upstream changes](/tricks/concepts/upstream/) into it.
