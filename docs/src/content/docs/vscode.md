---
title: VS Code extension
description: Discover, preview, try, link, experiment, merge upstream changes, lint and publish skills from VS Code, Cursor, Windsurf or VSCodium.
---

The New Tricks extension puts the source repo workflow in the editor: a Discover view for finding skills, a Source Repo view for your skills and their branches, a Links view for what your agents load, lint results in the Problems panel, VS Code's own diff and merge editors for upstream changes, and a publish pre-flight panel.

The extension is a thin client. It runs `tricks serve --stdio` and every action goes through the same core as the CLI, so anything you do in the editor you can also do with [`tricks`](/tricks/reference/commands/), and the other way round.

## Install

Install **New Tricks** (`newtricks.new-tricks`) from the VS Code Marketplace, or from Open VSX in Cursor, Windsurf and VSCodium:

```bash
code --install-extension newtricks.new-tricks
```

The platform builds bundle the `tricks` binary, so you don't need to install the CLI separately. The extension looks for the binary in this order:

1. The `tricks.path` setting, if set.
2. The binary bundled with the extension.
3. `tricks` on your `PATH`.

The server starts in the first folder of your workspace. Open your [source repo](/tricks/concepts/source-repo/) as the first folder, or on its own, to get the Source Repo view.

GitHub credentials are borrowed the same way as the CLI: `GITHUB_TOKEN`, then `gh auth token`. If neither is available, the extension passes VS Code's GitHub session to the server, in memory only. Turn that off with `tricks.useGitHubSession`.

## Settings

| Setting | Default | Effect |
|---|---|---|
| `tricks.path` | empty | Path to the `tricks` binary. Empty uses the bundled binary, then `tricks` on `PATH` |
| `tricks.offline` | `false` | Never touch the network (search the local index only) |
| `tricks.checkIntervalMinutes` | `60` | How often to check vendored skills for upstream changes while VS Code is open (at least 5). Fetches are still throttled by `fetch_interval` in your user config |
| `tricks.useGitHubSession` | `true` | Pass VS Code's GitHub session to `tricks` when `gh` isn't signed in |

Changing `tricks.path` or `tricks.offline` restarts the server.

## Views

The extension adds a **New Tricks** view container to the activity bar with three views.

### Discover

Search across your catalogs and the live catalogs as you type, with facets for agent, trust, licence and **no scripts**. It's the same search as [`tricks search`](/tricks/concepts/discovery/). Each result card shows the skill's name, ID and description, plus tags: trust, licence class, installs, stars, how many catalogs list it, identical copies, variants, Tessl quality, risk flags, and whether you've already linked or vendored it.

Each card has four actions:

- **Preview** opens the skill read-only (see below).
- **Try…** links it into a project without vendoring it, like `tricks try`.
- **Vendor** copies it into your source repo, like `tricks vendor`. If the folder isn't a source repo yet, the extension offers to initialize one.
- **Copy ID** copies the skill ID.

### Preview

**Preview Skill** opens any skill, by ID or GitHub URL, in the Markdown preview without cloning anything, and then shows its trust, licence and risk summary with **Try in Project…**, **Vendor** and **Files…** buttons. **Files…** (**Open Supporting File…**) lists the skill's other files. Scripts open as plain text, and nothing is ever run.

### Source Repo

The Source Repo view appears when the workspace has a source repo. It lists your skills with badges: `updating`, `update <ref>`, `customized`, lint errors, `using <branch>`, `editing <branch>`, `uncommitted`, and the number of branches. Clicking a skill opens its `SKILL.md`. Expand a skill to see:

- Where it came from: its upstream, or `local original`.
- **upstream has … — update**, when an update is ready, and **continue** or **abort** while an update is in progress.
- **show changes…** for vendored skills.
- Each branch of the skill, marked `(in use)` or `(editing)`. Clicking a branch runs **Use Variant…** for it, and its inline button runs **Merge Branch…**.
- The number of links.

The title bar has **Refresh**, **Create Skill…**, **Link Source Repo Skills**, **Lint Source Repo** and **Publish…**, plus **Check Upstream Changes** in its menu. Right-click a skill for the rest.

### Links

The Links view shows this source repo's links under **Source repo skills** and all your trials under **Trying**. It's the same information as [`tricks list --links` and `tricks list --trials --all`](/tricks/concepts/links-and-trials/). Each item shows the skill and agent, then where it is, what it deploys, the mode and any health problem:

```text
changelog-writer · claude     user-level · main (live) · link
changelog-writer · claude     my-app · terse (draft, pinned) · link
changelog-writer · claude     my-app · terse @ 195438a (pinned) · link
anthropics/skills//skills/webapp-testing · claude     my-app · link
```

An item's inline button unlinks it, or removes it with untry if it's a trial. The title bar has **Unlink This Source Repo's Skills** and **Remove All Trials**. When there's nothing linked, the view offers **Link Source Repo Skills** and **Initialize a source repo**.

## Link to a project

**Link to Project…** (on a skill's context menu) links one skill into a project folder. If the skill has branches, it first asks what the link should deploy:

- **default** follows the skill's default: the working tree, or its `use` variant.
- A branch pins the link to that branch, like `tricks link <skill>@<branch>`. The branch you're editing is marked `draft (live)` and the others `branch tip`.

Then you pick the agents: Claude Code, Codex, GitHub Copilot or Cursor, with Claude Code preselected. **Try in a Project…** asks for a project and agents the same way. If there's exactly one workspace folder besides the source repo, both commands use it without asking. If a skill of the same name is already in the project, you're offered **Shadow**, which backs up the existing skill and restores it when you unlink (`--shadow`). See [What a link deploys](/tricks/concepts/links-and-trials/#what-a-link-deploys).

## Experiment on a branch

The skill context menu covers [branch experiments](/tricks/concepts/branch-experiments/):

- **Experiment on a Branch…** asks for a branch (suggesting the one you're editing, or `draft/<skill>`), checks it out in `.tricks/work/`, and opens the draft's `SKILL.md`. Use **Link to Project…** with that branch to test the draft.
- **Commit Draft…** commits the draft on its branch (`tricks edit --commit`).
- **Finish Editing** ends the experiment without merging (`tricks edit --done`).
- **Use Variant…** picks **default** or a branch, then **Everyone** (`tricks.toml`, committed) or **This machine only** (`tricks.work.toml`).
- **Merge Branch…** picks a branch, then **Merge \<skill\> only**, **Merge the whole branch**, **Pull request for \<skill\>** or **Pull request for the whole branch**. If a merge stops on conflicts, the extension opens Source Control so you can resolve them and commit.

## Upstream changes

- **Check Upstream Changes** fetches every vendored skill's upstream and lists those with changes, with their incoming commits and risk flags. Pick one to update it. The extension also checks in the background every `tricks.checkIntervalMinutes`.
- **Show Changes…** opens VS Code's diff editor. For a vendored skill you can pick **My customizations** (base to working copy), **Incoming upstream** (base to latest upstream), **Candidate merge** (working copy to merge result, with nothing applied), **Uncommitted** (HEAD to working copy), or **Branch \<name\>** (HEAD to that branch). Local originals offer **Uncommitted** and their branches.
- **Update from Upstream** runs `tricks update` for one skill. A clean merge is left uncommitted, and **Review in Source Control** takes you there. Your agents keep the previous version until you commit. If there are conflicts, each text conflict opens in the three-way merge editor, with **Yours** and **Upstream** on either side of the base. Then run **Continue Update**, or **Abort Update** to restore your version. Binary and deleted-file conflicts are reported for you to resolve by hand.
- **Contribute Upstream…** offers your change to a vendored skill upstream as a pull request (`tricks contribute`).

See [Upstream tracking](/tricks/concepts/upstream/).

## Lint

Lint results appear in the Problems panel with source `new-tricks` and the rule code, for example `NT305`. The extension lints when it refreshes and whenever you save a file in the source repo. **Lint Source Repo** runs lint and opens the Problems panel, and **Lint and Apply Safe Fixes** runs `tricks lint --fix`. See [Lint](/tricks/concepts/lint/) and the [rule list](/tricks/reference/lint-rules/).

## Publish pre-flight

**Publish…** needs a `[publish.targets.<name>]` in `tricks.toml`. With more than one target, pick one. The panel runs a dry run first and shows:

- Each gate as passed, warning or failed, with details.
- The current version and the suggested bump, with **untagged**, **patch**, **minor** and **major** to choose from.
- **push to the target** or **open a pull request on the target**, and **accept strong copyleft**.
- The changes the target will receive, and a changelog preview.

**Publish** is disabled while a gate fails. See [Publishing](/tricks/concepts/publishing/).

## Status bar

One status bar item summarizes the source repo: pending upstream updates, an update in progress, lint errors (or warnings) and the number of links and trials, for example `3 upstream · 1 lint · 4`, each with its icon. With nothing to report, it shows **New Tricks**. Click it for quick actions: search, link, check upstream changes, lint, publish, unlink, remove trials, doctor and refresh.

## Editing assistance

- **`tricks.toml`**: the extension contributes a JSON schema for `tricks.toml`, for completion and validation in TOML extensions that support schemas.
- **`SKILL.md` frontmatter**: completion for the Agent Skills keys (`name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools`) and the Claude Code keys (`disable-model-invocation`, `argument-hint`, `model`), with snippets and hover documentation. Hovering over a key that isn't in the spec explains that it's kept and reported as NT402.

## Commands

All commands are in the Command Palette under **New Tricks**.

| Task | Commands |
|---|---|
| Discover | Search Skills, Preview Skill, Open Supporting File…, Try in a Project…, Vendor into Source Repo |
| Source repo | Initialize Source Repo Here, Create Skill… (new or from a folder), Remove Skill…, Open SKILL.md |
| Links | Link Source Repo Skills, Link to Project…, Unlink / Untry, Unlink This Source Repo's Skills, Remove All Trials |
| Experiments | Experiment on a Branch…, Commit Draft…, Finish Editing, Use Variant…, Merge Branch… |
| Upstream | Check Upstream Changes, Show Changes…, Update from Upstream, Continue Update, Abort Update, Contribute Upstream… |
| Lint and publish | Lint Source Repo, Lint and Apply Safe Fixes, Publish… |
| Maintenance | Refresh, Doctor (writes `tricks doctor` output to the **New Tricks** output channel), Status Actions |

When the core needs your confirmation, for example for a licence warning when vendoring, the extension shows it in a dialog with the details, and continues only if you choose **Continue**. Server messages and errors go to the **New Tricks** output channel.
