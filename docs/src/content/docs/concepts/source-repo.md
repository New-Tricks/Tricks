---
title: Source repo
description: The git repository where you author, customize, test and publish skills, and the commands that add, remove and report on its skills.
---

A **source repo** is a git repository where skills are authored, customized, tested and published from. It holds your own skills (originals) and customized copies of upstream skills (vendored skills), plus anything else you keep next to them: scripts, notes, evals. It is the one thing New Tricks operates on.

You own the source repo and its remote. New Tricks commits only when you ask it to (`edit --commit`, `merge`) and never pushes it, except when you ask for a pull request with `merge --pr`. Most changes it makes, such as adding or removing a skill or merging upstream changes, are left uncommitted for you to review with git.

## Make a repository a source repo

Run [`tricks init`](/tricks/reference/commands/init/) in any git repository. Outside one, it runs `git init` first.

```bash
cd ~/code/my-skills
tricks init
```

```text
initialized a git repository in ~/code/my-skills
created source repo `my-skills` at ~/code/my-skills
next: `tricks create <name>` or `tricks vendor owner/repo//skill`, then `tricks link`
```

`init` does four things:

- Writes `tricks.toml`, the manifest, with commented examples, and an empty `tricks.lock`.
- Creates the `skills/` folder.
- Adds `tricks.work.toml` and `/.tricks/` to `.gitignore`.
- Registers the repo in your user config under `[source-repos]`, keyed by its name. The name defaults to the directory name; `--name` sets `[source-repo] name` instead.

Running `init` again in an existing source repo only registers it (useful on a new machine after cloning).

`tricks init --agent-skill` also installs the bundled `new-tricks` agent skill into the repo's project-level agent directories (for example `.claude/skills/new-tricks`), excluded from git like any link. Agents working in the repo then know how to use New Tricks safely. See [Working with coding agents](/tricks/guides/working-with-coding-agents/).

## Layout

```text
my-skills/
  tricks.toml            # manifest: skills, upstreams, agents, lint, publish targets (committed)
  tricks.lock            # generated: base revisions and licences of vendored skills (committed)
  tricks.work.toml       # machine-local overrides, e.g. `use --local` (git-ignored)
  skills/
    changelog-writer/
      SKILL.md
    skill-creator/
      SKILL.md
      scripts/…
  .tricks/work/<branch>/ # worktrees of branch experiments (git-ignored)
```

- **`tricks.toml`** records each skill's path and, for vendored skills, its upstream, what it tracks and its update policy. It also holds the agents to link for, lint settings and publish targets. You can edit it by hand; New Tricks preserves your formatting when it edits it.
- **`tricks.lock`** records facts, not intent: for each vendored skill, the upstream revision it last incorporated (the base) and the detected licence. Don't edit it.
- **`tricks.work.toml`** holds choices that apply to your machine only, so personal experiments never change the committed manifest.
- **`.tricks/work/`** holds the worktrees [`edit`](/tricks/reference/commands/edit/) checks experiments out into, one per branch (`draft/changelog-writer` becomes `.tricks/work/draft--changelog-writer/`). See [Branch experiments](/tricks/concepts/branch-experiments/).

Skills live in `skills/<name>/` by default, but any path works; `vendor --path` puts a skill elsewhere. Every key is described in the [configuration reference](/tricks/reference/configuration/).

## Add skills

### Create your own

[`tricks create`](/tricks/reference/commands/create/) scaffolds a new original skill with a `SKILL.md` whose frontmatter has the name and your description:

```bash
tricks create changelog-writer --description "Writes release notes from merged pull requests and commit history. Use when the user asks for a changelog, release notes or a summary of what shipped."
```

Or bring in an existing folder, such as a skill you wrote before using New Tricks. `--from` copies it into `skills/<name>/`:

```bash
tricks create release-notes --from ~/old/release-notes
```

Either way the skill is an original: no upstream. The files and manifest changes are left uncommitted.

### Vendor an upstream skill

[`tricks vendor`](/tricks/reference/commands/vendor/) copies an upstream skill into the repo to customize it:

```bash
tricks vendor anthropics/skills//skill-creator
```

It records the upstream in `tricks.toml` (with `track = "latest"` and `update = "review"`) and the base revision and licence in `tricks.lock`, then shows the licence class and risk surface. Skills whose licence may forbid modification ask for confirmation. Catalog-hosted skills (ClawHub, `.well-known` indexes) can be vendored too, and `vendor --from <copy> --base <rev>` records a copy you made earlier at the revision it started from. Merging later upstream changes is covered in [Upstream tracking](/tricks/concepts/upstream/).

:::tip
You don't have to vendor a skill to evaluate it. [`tricks try`](/tricks/reference/commands/try/) puts an upstream skill in front of your agents first; vendor it once you decide to customize it.
:::

## Remove a skill

[`tricks remove`](/tricks/reference/commands/remove/) removes the skill's links, deletes its folder, and drops it from `tricks.toml` and `tricks.lock`. The change is left uncommitted. If the skill has uncommitted changes, it asks for confirmation first (`--yes` confirms):

```bash
tricks remove release-notes
```

```text
removed release-notes (skills/release-notes); not committed yet
```

## See the state of your skills

[`tricks list`](/tricks/reference/commands/list/) shows each skill with notes on its state. It reads only local state and caches, so it is fast and works offline. (Paths in the output on this page are shortened with `~`; `tricks` prints them in full.)

```bash
tricks list
```

```text
source repo my-skills (~/code/my-skills, branch main)
  changelog-writer       original · editing on terse · branches: terse · linked: ~/code/my-app (terse), user level (main)
  skill-creator          from anthropics/skills//skills/skill-creator · customized · upstream has v1.1.0 · linked: user level (main)
  publish targets: public
```

The notes, in the order they appear:

| Note | Meaning |
|---|---|
| `original` | You wrote it; it has no upstream. |
| `from <upstream>` | Vendored from this upstream skill. |
| `customized` | The skill's files differ from the upstream base it was vendored or last updated from. |
| `upstream has <version>` | A newer upstream revision is in the local cache. Run [`tricks outdated`](/tricks/reference/commands/outdated/) to fetch and see the changes, [`tricks update`](/tricks/reference/commands/update/) to merge them. |
| `UPDATE IN PROGRESS` | An upstream update stopped on conflicts. Resolve them, then `tricks update --continue` (or `--abort`). |
| `uncommitted` | The skill's folder has changes git hasn't committed. |
| `lint <n>E/<n>W` | Lint errors and warnings. Run [`tricks lint`](/tricks/reference/commands/lint/) for details. |
| `using <branch>` | Links follow this branch by default, chosen with [`tricks use`](/tricks/reference/commands/use/) (committed, or `--local`). |
| `editing on <branch>` | An experiment is checked out for this skill on this branch in `.tricks/work/`. |
| `branches: <names>` | Other local branches that changed this skill since they branched off the current one. A branch merged with `tricks merge` stays listed until you delete it, because the merge brings the changes over as a new commit. |
| `linked: <place> (<branch>), …` | Where the skill is linked, and the branch each link deploys: `user level` for the user-level agent directories, or a project path. |

`publish targets:` lists the targets in `[publish.targets]`. Upstream changes shown by `list` come from caches only; `outdated` is the command that fetches.

`tricks list --json` gives the same information in machine-readable form. Each skill carries `name`, `path`, `upstream`, `base`, `track`, `customized`, `update_available`, `license`, `lint_errors`, `lint_warnings`, `branches`, `variant`, `editing`, `merge_in_progress`, `dev_links` and `uncommitted`; the report also includes `repos`, `links`, `trials` and `unfinished_operations`.

`list --links` and `list --trials` show links and trials instead; see [Links and trials](/tricks/concepts/links-and-trials/).

## Registered source repos

Every repo you `init` is registered in your user config (`~/.config/newtricks/tricks.toml`):

```toml
[source-repos]
my-skills = "~/code/my-skills"
team      = "~/code/acme-skills"
```

Most people have one personal and one team source repo. Outside a source repo, `tricks list` shows the registered ones:

```bash
cd ~ && tricks list
```

```text
source repos:
  my-skills              2 skill(s)  ~/code/my-skills
```

Registration is also how `unlink --all` and `list --links --all` find every source repo's links, and how `doctor` checks each repo still exists. To unregister a repo, delete its line from the user config.

## Commands run from a draft act on the repo

Branch experiments live in worktrees at `.tricks/work/<branch>/`, each a full checkout with its own `tricks.toml`. New Tricks maps a worktree back to the source repo it belongs to, so commands you run inside a draft (for example from `tricks edit --shell`, or an agent working there) act on the source repo:

```bash
cd "$(tricks edit skill-creator)"
tricks list
```

```text
source repo my-skills (~/code/my-skills, branch main)
  changelog-writer       original · branches: terse
  skill-creator          from anthropics/skills//skills/skill-creator · customized · lint 0E/1W · editing on draft/skill-creator
```

## Related

- [Upstream tracking](/tricks/concepts/upstream/): keep vendored skills current.
- [Branch experiments](/tricks/concepts/branch-experiments/): drafts, `use` and `merge`.
- [Links and trials](/tricks/concepts/links-and-trials/): test skills with real agents.
- [Configuration reference](/tricks/reference/configuration/): every key in `tricks.toml`, `tricks.lock` and `tricks.work.toml`.
