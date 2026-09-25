---
title: Branch experiments
description: Try a change to a skill on a git branch, test the draft with agents, compare it, and merge only the skill back.
---

An experiment is a git branch of your [source repo](/tricks/concepts/source-repo/), checked out in its own worktree next to your skills. You edit the draft there, [link](/tricks/concepts/links-and-trials/) it into a project to test it with an agent, commit it, compare it with `main`, and merge the skill back, or throw the branch away. Your main checkout, and every link that isn't pinned to the branch, stays as it was.

```bash
tricks edit changelog-writer -b terse                     # start: branch terse, worktree in .tricks/work/terse
tricks link changelog-writer@terse --to ~/code/my-app     # this project's agents load the draft
tricks edit changelog-writer --commit -m "Terser output"  # commit the draft on its branch
tricks edit changelog-writer --done                       # stop editing; pinned links deploy the branch tip
tricks diff changelog-writer head..terse                  # compare
tricks merge changelog-writer@terse                       # bring back only the skill's folder
```

For a step-by-step walkthrough, see [Test a draft with agents](/tricks/guides/test-a-draft-with-agents/).

## Start an experiment

[`tricks edit <skill>`](/tricks/reference/commands/edit/) always works on a branch:

- With `-b <branch>` (or `--branch`), that branch. It's created from the current `HEAD` if it doesn't exist.
- Without `-b`, the experiment already under way for that skill. If there isn't one, `draft/<skill>`.

The branch is checked out as a git worktree inside the source repo at `.tricks/work/<branch>`, with `/` in the branch name replaced by `--`. So `terse` is at `.tricks/work/terse` and `draft/changelog-writer` is at `.tricks/work/draft--changelog-writer`. The first `edit` adds `/.tricks/` to `.gitignore`, so drafts never show up as changes. Because drafts live inside the repo, they sit next to your skills in the editor, and a coding agent working in the repo can write to them.

`edit` prints the draft's path on stdout and its hints on stderr, so you can `cd` straight into it:

```bash
cd "$(tricks edit changelog-writer)"
```

```text
~/code/my-skills/.tricks/work/terse/skills/changelog-writer
editing `changelog-writer` on branch terse (`cd "$(tricks edit changelog-writer)"` or `--shell` to work there); commit drafts with `tricks edit changelog-writer --commit -m "…"`, then `tricks merge changelog-writer@terse`
to try the draft with agents: `tricks link changelog-writer@terse`
```

`--shell` opens your `$SHELL` in the draft (`%COMSPEC%` on Windows) and sets `TRICKS_EDITING=<skill>@<branch>`, for example `TRICKS_EDITING=changelog-writer@terse`, which you can show in your prompt. Type `exit` to return. `tricks` commands you run inside a draft act on its source repo.

## Test the draft with agents

`edit` doesn't re-point any links. Your user-level link keeps deploying `main`. To have an agent load the draft, pin a link to the branch:

```bash
tricks link changelog-writer@terse --to ~/code/my-app
```

```text
linked changelog-writer into ~/code/my-app from terse (draft, live, pinned)
  claude   ~/code/my-app/.claude/skills/changelog-writer (link)
```

While you're editing, the link points into the worktree, so every save shows up in the agent. [What a link deploys](/tricks/concepts/links-and-trials/#what-a-link-deploys) explains how pinned links behave afterwards.

## Commit the draft

```bash
tricks edit changelog-writer --commit -m "Terser output"
```

```text
committed changelog-writer 195438adc on terse
```

`--commit` stages and commits only the skill's folder, and only on the branch being edited. When it detects a coding agent from its environment (Claude Code, Codex, Cursor or Copilot), it adds a `Tricks-Agent:` trailer, so you can tell agent commits from your own:

```text
committed changelog-writer e689074f1 on draft/changelog-writer (Tricks-Agent: claude-code)
```

You can also commit in the worktree with plain git. New Tricks picks up whatever is on the branch.

## Finish editing

```bash
tricks edit changelog-writer --done
```

```text
~/code/my-skills/.tricks/work/terse/skills/changelog-writer
finished editing `changelog-writer`; links pinned to terse deploy its last commit
links on terse:
  → ~/code/my-app/.claude/skills/changelog-writer (link)
```

`--done` ends the experiment without merging. Links pinned to the branch switch from the live draft to a snapshot of the branch tip, shown as `terse @ 195438a (snapshot, pinned)` in `tricks list --links`. If the draft still has uncommitted changes, `--done` warns you. Commit first, because the snapshot only has committed work. The branch and its worktree stay, so `tricks edit changelog-writer -b terse` picks up where you left off.

## Compare versions

[`tricks diff <skill> [<from>..<to>]`](/tricks/reference/commands/diff/) compares two versions of a skill inside the source repo:

| Name | Version |
|---|---|
| A branch or commit | The skill as committed there, for example `terse` or `3f2a1c9` |
| `head` | The skill at `HEAD` of your main checkout |
| `working` | The skill in your main checkout's working tree |
| `base` | The upstream revision a vendored skill was last updated from |

The default range is `head..working`. `a..` means `a..working`, and `..b` means `head..b`.

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

A branch name means its committed tip. `working` is always your main checkout, even when you run `diff` inside a draft. To see uncommitted changes in a draft, run `git diff` in the draft. `diff` doesn't compare with upstream: it points you to [`outdated --diff` and `update --dry-run`](/tricks/concepts/upstream/) instead.

## Choose the default with `use`

A link that isn't pinned follows the skill's default: your working tree. [`tricks use <skill>@<branch>`](/tricks/reference/commands/use/) makes a branch the default for every unpinned link of the skill, for example to live with a variant for a while before you merge it.

```bash
tricks use changelog-writer@terse             # recorded in tricks.toml (committed, for everyone)
tricks use changelog-writer@default --local   # this machine only, in tricks.work.toml (git-ignored)
tricks use changelog-writer --reset           # back to the working tree
```

```text
changelog-writer now uses terse
  → ~/code/my-app/.claude/skills/changelog-writer (link)
  → ~/.claude/skills/changelog-writer (link)
```

`use` writes `use = "terse"` under `[skills.changelog-writer]` in `tricks.toml`. With `--local`, the choice goes under `[use]` in `tricks.work.toml` and overrides the committed one on your machine. `@default` means the working tree. `--reset` clears the committed choice and the local override. Links that follow a `use` branch deploy a snapshot of its tip, such as `terse @ fc8aeed (snapshot)`. If that branch is checked out, they deploy the working tree, and while you're editing it, the draft.

## Merge it back

[`tricks merge <skill>@<branch>`](/tricks/reference/commands/merge/) brings an experiment into the branch your source repo is on. By default it takes only the skill's folder: it applies the skill's changes since the branch point as a three-way patch and commits them as one commit, so later changes on your current branch are kept.

```bash
tricks merge changelog-writer@terse
```

```text
merged changelog-writer from terse into main (8ffacb0bb)
  → ~/code/my-app/.claude/skills/changelog-writer (link)
  → ~/.claude/skills/changelog-writer (link)
  note: terse also changes 1 file(s) outside changelog-writer (not merged; use --whole-branch to include them)
```

| Option | Effect |
|---|---|
| `--whole-branch` | Merge the entire branch with `git merge --no-ff`. Your working tree must be clean |
| `--pr` | Push to the source repo's `origin` and open a pull request with `gh`. For a skill-only merge, New Tricks pushes a fresh `tricks/merge-<skill>-<branch>` branch holding just those changes. Nothing else changes until the pull request lands |
| `-m <message>` | The commit or pull request message. The default is `Merge terse into changelog-writer` (or `Merge branch 'terse'`) |

After a local merge:

- Editing that branch ends.
- A `use` of that branch, in `tricks.toml` or `tricks.work.toml`, is reset.
- Links pinned to the branch are un-pinned and follow the default, which now has the changes. They show `main (working tree, live)` again.

`merge` refuses to run while the draft you're editing on that branch has uncommitted changes, or while the skill has uncommitted changes on your current branch.

### Conflicts

If the changes don't apply cleanly, `merge` stops and leaves the conflicts for git:

```text
merging changelog-writer from verbose into main stopped on conflicts:
    CONFLICT skills/changelog-writer/SKILL.md
  resolve them, then commit with git
```

Resolve the conflict markers and commit with git (`git merge --continue` for `--whole-branch`). The steps New Tricks takes after a clean merge don't run in this case, so finish them yourself: `tricks edit <skill> --done` if you were editing the branch, and `tricks link <skill> --to <project>` to un-pin each link that was pinned to it.

### Clean up the branch

The worktree and the branch stay after a merge. Remove them with git. A skill-only merge is a patch, not a git merge, so git considers the branch unmerged and you need `-D`:

```bash
git worktree remove .tricks/work/terse
git branch -D terse
```

## Using git directly

Experiments are ordinary branches and worktrees. You can commit, rebase, cherry-pick or push them with git or your editor, and New Tricks picks up the result. A snapshot link to a branch that moved is refreshed by the next `tricks` command you run in the source repo, or when the VS Code extension refreshes.
