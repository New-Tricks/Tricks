# Changelog

## 0.5.0

- The Links view shows this source repo's links and all trials separately; items unlink or untry as appropriate, and **Remove All Trials** joins **Unlink This Source Repo's Skills**.
- **Experiment on a Branch…** (was *Edit on Branch…*) suggests `draft/<skill>`; drafts are checked out inside the repo in `.tricks/work/`.

## 0.4.0

- **Create Skill…** (new, or from a folder) replaces *New Skill…*; **Remove Skill…** takes a skill out of the source repo.
- **Commit Draft…** commits a draft on the branch you're editing; **Merge Branch…** brings it back — just the skill or the whole branch, locally or as a pull request. Branch items in the Source Repo view offer it inline.
- Upstream: **Sync with Upstream** (was *Merge Upstream Changes*), **Continue Sync** / **Abort Sync**; **Check Upstream Changes** uses `tricks outdated`.
- **Show Changes…** can compare a skill with any of its branches.
- Preview reads skill details with `tricks info`; *Try* uses `tricks try`.

## 0.3.0

New Tricks now works on one thing: the **source repo**. Installing and updating skills on your machine is left to APM, `npx skills` and plugin marketplaces.

- **Links** view (was *Installed & Links*): your source repo's skills linked for your agents, and upstream skills you are trying, with unlink on each.
- **Link Source Repo Skills** links every skill for your agents (edits are live); **Link to Project…** links one into a project.
- Discover: **Try…** links an upstream skill into a project without vendoring it (replaces *Install*); the *installed* filter is gone and a *linked* tag shows trials.
- **Check Upstream Changes** and **Merge Upstream Changes** (was *Update*); ClawHub and `.well-known` skills can be vendored and merged too.
- **Finish Editing** replaces *Commit Skill*: commit with git, then finish.
- **Contribute Upstream…** replaces *Open Pull Request Upstream…*.
- Removed: *Install for Agents*, *Remove from User Skills*, *Review User Skill Updates*, *Roll Back*, *Install Agent Skill* and the first-run agent-skill offer (install it with `npx skills add new-tricks/tricks`, or `tricks init --agent-skill` in a source repo).

## 0.2.1

- Discover: live catalogs are searched concurrently, so cold searches finish roughly twice as fast.
- `tricks.search` accepts a query argument and hands it to the Discover view.
- Publish page: text embedded in the page script is escaped.

## 0.2.0

- Terminology: the Workspace view is now **Source Repo**, workbench skills are **user skills**, and search sources are **catalogs**. Command ids follow (`tricks.initSourceRepo`, `tricks.userUpdate`).
- Publish panel: choose between pushing to the target and opening a pull request (one is required).

## 0.1.0

- Discover: federated, faceted skill search with trust, licence and risk signals.
- Preview any skill read-only (Markdown preview, supporting files as text).
- Workspace view: vendored skills, upstream merges in the three-way merge editor, branch experiments, variants, test links.
- Lint diagnostics in the Problems panel.
- Publish pre-flight panel.
- Status bar: updates, merges, lint, active test links.
