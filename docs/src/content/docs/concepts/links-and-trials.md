---
title: Links and trials
description: How New Tricks puts skills in front of real agents for testing, with link for your source repo's skills and try for skills from elsewhere.
---

The only way to know whether a skill works is to watch an agent use it. New Tricks places skills into the directories agents load skills from, for testing. It has two sets of commands for this:

- [`link`](/tricks/reference/commands/link/) and [`unlink`](/tricks/reference/commands/unlink/) for skills in your [source repo](/tricks/concepts/source-repo/): your own skills and your vendored copies.
- [`try`](/tricks/reference/commands/try/) and [`untry`](/tricks/reference/commands/untry/) for skills from anywhere else: an upstream skill you found with [discovery](/tricks/concepts/discovery/), or a skill folder outside the repo.

Links are for testing, not installation. New Tricks doesn't manage what's installed on your machine. APM, `npx skills` and plugin marketplaces do that, and New Tricks works alongside the skills they install.

## Two commands, two kinds of skill

| | `link <skill>` | `try <skill>` |
|---|---|---|
| Takes | A skill of the current source repo, or `<skill>@<branch>` | An upstream ID (`owner/repo//skill`, a URL, a ClawHub or `.well-known` ID) or a local folder |
| Deploys | The skill's default, or the branch the link is pinned to (see below) | An exact revision from the store. A local folder is linked as is, so edits show up live |
| Default target | The user-level agent directories | The current project |
| Changes later | Follows `use`, `merge`, `update` and branch moves | Never. A trial is never locked or updated |
| Listed by | `tricks list --links` | `tricks list --trials` |
| Removed by | `tricks unlink` | `tricks untry` |

The sets are kept apart so that each list and each removal means one thing. `unlink` never touches a trial, and `untry` never touches a link of your own skill. If you use the wrong command, it tells you which one to use:

```text
error: `changelog-writer` is a skill of this source repo; link it with `tricks link changelog-writer`
error: `webapp-testing` is a trial; remove it with `tricks untry webapp-testing`
error: not inside a source repo; to try `webapp-testing`, use `tricks try webapp-testing`
```

## Link source repo skills

Run `link` inside the source repo. With no skill, it links every skill of the repo into the user-level agent directories:

```bash
cd ~/code/my-skills
tricks link                                           # every skill, user level
tricks link changelog-writer --to ~/code/my-app       # one skill, into one project
tricks link changelog-writer --agents claude,cursor   # pick the agents
```

```text
linked changelog-writer into the user-level agent directories from main (working tree, live)
  claude   ~/.claude/skills/changelog-writer (link)
linked skill-creator into the user-level agent directories from main (working tree, live)
  claude   ~/.claude/skills/skill-creator (link)
see links with `tricks list --links`; remove them with `tricks unlink`
```

| Flag | Effect |
|---|---|
| `--to <project>` | Link into that project's agent directories (for example `.claude/skills/`) instead of the user-level ones |
| `--global` | Link into the user-level agent directories (this is already the default for `link`) |
| `--agents <list>` | Choose which agents to link for, as a comma-separated list or `all`. The default comes from the source repo's `agents` setting, else your user setting. See [Agents](/tricks/concepts/agents/) |
| `--copy` | Copy instead of symlinking (see [Link or copy](#link-or-copy)) |
| `--shadow` | Replace a skill of the same name that is already there, and restore it on unlink (see [Collisions](#collisions-and---shadow)) |

## What a link deploys

Every link of a source repo skill deploys one branch, and links don't have to agree. A project can test the `terse` experiment while your user-level link stays on `main`.

- **`link <skill>`** follows the skill's **default**. That's the working tree of your checkout, live, so agents see an edit as soon as you save it. If you picked a variant with [`use`](/tricks/concepts/branch-experiments/#choose-the-default-with-use), it's that branch instead.
- **`link <skill>@<branch>`** **pins** that link to a branch. What it deploys depends on the state of the branch:
  - While [`edit`](/tricks/concepts/branch-experiments/) has the branch checked out, the link deploys the **draft**, live.
  - If the branch is the one checked out in the source repo, the link deploys the **working tree**.
  - Otherwise, the link deploys a **snapshot** of the branch tip from the store. When the branch moves, the next `tricks` command you run in the source repo refreshes the snapshot. A VS Code extension refresh does too.

`tricks list --links` shows each link grouped by place, with the branch it deploys and how it gets there:

```text
user level:
  changelog-writer             claude   ~/.claude/skills/changelog-writer (link)  main (working tree, live)
  skill-creator                claude   ~/.claude/skills/skill-creator (link)  main (working tree, live)
~/code/my-app:
  changelog-writer             claude   ~/code/my-app/.claude/skills/changelog-writer (link)  terse (draft, live, pinned)
```

After you commit the draft and finish editing, the same link shows a snapshot:

```text
~/code/my-app:
  changelog-writer             claude   ~/code/my-app/.claude/skills/changelog-writer (link)  terse @ 195438a (snapshot, pinned)
```

`tricks list` shows the same information for each skill: `linked: ~/code/my-app (terse), user level (main)`.

:::note
When a skill with the same name is at user level and in the project, the agent decides which one it loads. Claude Code uses the user-level one. To test a project link with Claude Code, remove the user-level link while you test. The guide [Test a draft with agents](/tricks/guides/test-a-draft-with-agents/) shows how.
:::

Pins stay until you change them:

- `tricks link <skill>` (with `--to` for a project link) un-pins that link, so it follows the default again.
- `tricks link` with no skill links every skill but keeps existing pins.
- A local [`merge`](/tricks/concepts/branch-experiments/#merge-it-back) of a branch moves links pinned to that branch back to the default, which now has the changes.
- If a pinned branch is deleted, the link stays as it was and every command prints a warning such as ``links of changelog-writer pinned to terse: no branch `terse` in source repo my-skills``. Re-link or unlink it.

While an [upstream update](/tricks/concepts/upstream/) rewrites a vendored skill in your working tree, its links keep deploying the last committed version. They go back to the working tree once you commit the merge result.

## Try skills from elsewhere

`try` links a skill that isn't in your source repo, so you can evaluate it before [vendoring](/tricks/concepts/upstream/) it. It works anywhere, and by default it targets the current project:

```bash
cd ~/code/my-app
tricks try anthropics/skills//webapp-testing   # an exact revision, into this project
tricks try ~/old/pr-reviewer --global          # a local folder, user level, live
tricks list --trials                           # trials here and at user level
tricks untry webapp-testing                    # one trial, wherever it is
```

```text
trying webapp-testing into ~/code/my-app
  claude   ~/code/my-app/.claude/skills/webapp-testing (link)
see trials with `tricks list --trials`; remove with `tricks untry webapp-testing`
```

An upstream trial is an exact revision in the store. For catalog-hosted skills, that revision is verified. The trial never changes on its own: to test a newer revision, `untry` it and `try` it again.

`untry <skill>` removes that trial wherever it is. With no skill, it removes the trials in the current project. `--global` and `--to <project>` pick another place, and `--all` removes every trial. `list --trials` shows the trials in the current project and at user level, and `--all` shows every trial.

## See and remove links

`tricks list --links` lists the current source repo's links. Outside a source repo, pass `--all` to list the links of every registered source repo.

`unlink` removes links of source repo skills:

```bash
tricks unlink                                   # all of this repo's links, everywhere
tricks unlink changelog-writer                  # one skill's links
tricks unlink changelog-writer --to ~/code/my-app   # only in that project (--global: only user level)
tricks unlink --all                             # every source repo's links (needed outside a repo)
```

## Git status stays clean

A link inside a git repository, like `~/code/my-app/.claude/skills/changelog-writer`, is added to that repository's local exclude file, never to `.gitignore`. New Tricks finds the file with `git rev-parse --git-path info/exclude`, so worktrees and submodules work too. The entries go in a marked block:

```text
# >>> new-tricks (managed; do not edit)
/.claude/skills/changelog-writer
# <<< new-tricks
```

Nothing shows up in `git status` and nothing gets committed by accident. `unlink` and `untry` remove an entry once no other link uses that path.

## Collisions and `--shadow`

If the target already has a skill with that name, for example one another tool installed, `link` and `try` refuse:

```text
error: ~/code/my-app/.claude/skills/changelog-writer already exists (not managed by New Tricks). Use --shadow to back it up and replace it; `unlink` restores it.
```

With `--shadow`, New Tricks moves the existing folder to `backups/` in its data directory, links in its place, and puts the original back when you unlink:

```text
backed up existing ~/code/my-app/.claude/skills/changelog-writer to ~/Library/Application Support/newtricks/backups/1790368005-claude-changelog-writer
...
restored original ~/code/my-app/.claude/skills/changelog-writer
```

## Link or copy

| Mode | What's placed | When |
|---|---|---|
| Symlink | A link to the working tree, a draft, or a store entry | The default where the agent follows links |
| Copy | A physical copy, written to a temporary directory and swapped into place | With `--copy`, and automatically for agents or platforms that can't follow links: always on Windows, always for GitHub Copilot. See [Agents](/tricks/concepts/agents/#link-or-copy) |

A copy of a working tree or draft doesn't update when you edit. Run `tricks link` again to refresh it. When an agent gets a copy of a live target, `link` warns you:

```text
warning: some selected agents cannot follow links here; they get a copy, so live edits will not show until you re-link
```

If you edit a copied trial in place, `list --trials` flags it as `drifted`.

## The store is a cache

Snapshots of pinned branches and `use` variants, trials, and the base snapshots of catalog-hosted upstreams live in the store: `store/<tree-hash>/` in the data directory. Entries are read-only and named by content, so nothing can silently change a revision an agent is using. The store is only a cache. `unlink` and `untry` delete the entries that no link needs any more.

## Link health

`list --links` and `list --trials` mark any link that isn't healthy with `!!`, and [`tricks doctor`](/tricks/reference/commands/doctor/) reports it under `placements`. New Tricks never silently re-applies a link.

| State | Meaning | What to do |
|---|---|---|
| `ok` | The link is in place | Nothing |
| `missing` | Something deleted the link | Run `link` or `try` again, or remove it |
| `replaced` | Something else is at the path now, such as another tool's install | Decide which to keep. `link` refuses to overwrite it, and `unlink` leaves it in place |
| `target-missing` | The link points at a folder that no longer exists | Re-link, or unlink |
| `project-missing` | The project directory is gone | `unlink <skill>` or `untry <skill>` without `--to` (which needs an existing directory) |
| `drifted` | A copied trial was edited in place | `untry` and `try` again |
