---
title: Introduction
description: What New Tricks is, the problem it solves for skill authors, and how it fits alongside APM, npx skills and plugin marketplaces.
---

New Tricks (command `tricks`) is the design-time workbench for agent skills. An agent skill is a folder with a `SKILL.md` that coding agents such as Claude Code, Codex, GitHub Copilot and Cursor load on demand. New Tricks helps you write those skills, customize other people's, test them with real agents, and ship them.

You work in a **source repo**: an ordinary git repository that holds your own skills and customized copies of upstream skills. New Tricks helps you find prior art, keep your customized copies merging upstream improvements, experiment on branches, put skills in front of real agents, lint them, and publish a clean distribution repository that every popular installer understands.

## The problem

If you write skills for coding agents, you run into the same few problems:

- **Skills are scattered.** Prior art lives in GitHub repositories, Claude plugin marketplaces, skills.sh, Tessl, ClawHub and elsewhere, each with its own search and its own idea of what a skill is. Finding a good starting point means searching them all.
- **Customizing upstream skills goes stale.** You copy a skill someone else wrote and adapt it to your team. Upstream keeps improving it, and your copy drifts. Merging those improvements by hand is tedious enough that nobody does it.
- **Testing needs real agents.** A skill is a prompt. The only real test is watching an agent use it, in a real project. You need to get a draft in front of Claude Code or Codex quickly, compare it with the current version, and take it out again without leaving a mess in the project's git status.
- **Shipping means many formats.** People install skills with APM, `npx skills`, Claude plugin marketplaces, or by copying folders. Each expects a slightly different layout and manifest.

## What New Tricks does

The core loop:

```text
discover ─► create / vendor ─► edit on a branch ─► link & try with agents ─► merge ─► lint ─► publish
```

1. **Discover.** [`tricks search`](/tricks/reference/commands/search/) runs one search over skill repositories, marketplaces, skills.sh, Tessl, ClawHub and GitHub code search, deduplicated by content. [`info`](/tricks/reference/commands/info/) and [`view`](/tricks/reference/commands/view/) show a skill's licence, risk and content before you bring it into your repo, and [`try`](/tricks/reference/commands/try/) puts it in front of your agents in the current project. See [Discovery](/tricks/concepts/discovery/).
2. **Create or vendor.** [`create`](/tricks/reference/commands/create/) scaffolds your own skill. [`vendor`](/tricks/reference/commands/vendor/) copies an upstream skill into your source repo and records where it came from, so later upstream changes can be merged into your version. See [Source repo](/tricks/concepts/source-repo/).
3. **Edit on a branch.** [`edit`](/tricks/reference/commands/edit/) checks out a skill on a git branch in a worktree inside the repo, so experiments never disturb the version you and your agents rely on. See [Branch experiments](/tricks/concepts/branch-experiments/).
4. **Link and try with agents.** [`link`](/tricks/reference/commands/link/) places source repo skills in agent skill directories, user-level or in one project, live or pinned to a branch. The project's `git status` stays clean. See [Links and trials](/tricks/concepts/links-and-trials/).
5. **Merge.** [`merge`](/tricks/reference/commands/merge/) brings an experiment back: only that skill's folder, as one commit, or as a pull request.
6. **Lint.** [`lint`](/tricks/reference/commands/lint/) checks spec conformance, structure, triggering quality, agent compatibility and safety. See [Lint](/tricks/concepts/lint/).
7. **Publish.** [`publish`](/tricks/reference/commands/publish/) writes a distribution repository with a Claude `marketplace.json`, `apm.yml`, provenance and changelog, gated on lint, licences and a leak check. See [Publishing](/tricks/concepts/publishing/).

Alongside the loop, [`outdated`](/tricks/reference/commands/outdated/) and [`update`](/tricks/reference/commands/update/) keep vendored skills current with a three-way merge that keeps your changes. See [Upstream tracking](/tricks/concepts/upstream/).

## What New Tricks is not

New Tricks does not manage the skills installed on your workstation, and it is not a package manager. It has no install command and never updates skills on your machine by itself.

**Links are for testing, not installation.** When you link a skill into `~/.claude/skills` or a project, you are testing it. When you are done, you unlink it, merge, and publish. The people who use your skills install them from the published repository with their usual tool.

## How it fits with other tools

New Tricks sits before the installers and feeds them:

| Stage | Tool |
|---|---|
| Find prior art, preview, try | New Tricks: `search`, `info`, `view`, `try` |
| Author, vendor, customize, experiment, test, lint | New Tricks, in a source repo |
| Publish a distribution repository | `tricks publish` |
| Install into workstations, projects and CI | [APM](https://github.com/microsoft/apm), `npx skills`, Claude plugin marketplaces |

One published repository installs with all of them: `npx skills add owner/repo`, `apm install owner/repo`, or `/plugin marketplace add owner/repo` in Claude Code. Copilot, Codex and Cursor read the same `skills/<name>/` layout. New Tricks also coexists with skills those tools have already placed: it refuses to overwrite a skill it did not put there unless you ask it to with `--shadow`.

## Key terms

| Term | Meaning |
|---|---|
| **Source repo** | A git repository with a `tricks.toml`, where you author, customize, test and publish skills. You own it and push it. See [Source repo](/tricks/concepts/source-repo/). |
| **Vendored skill** | A copy of an upstream skill in your source repo, with its upstream and base revision recorded so upstream changes can be merged. |
| **Original** | A skill you wrote in the source repo, with no upstream. |
| **Upstream** | The repository and path (or catalog entry) a vendored skill came from. See [Upstream tracking](/tricks/concepts/upstream/). |
| **Link** | A source repo skill placed in an agent's skill directory for testing: a symlink, or a copy for agents that cannot follow links. See [Links and trials](/tricks/concepts/links-and-trials/). |
| **Trial** | A skill that is not in your source repo, placed in an agent directory with `try` to evaluate it: an upstream skill at a fixed revision, or a local folder. |
| **Draft** | A skill checked out on an experiment branch in `.tricks/work/<branch>/`. See [Branch experiments](/tricks/concepts/branch-experiments/). |
| **Variant** | A branch of a skill chosen with `use` as the version its links deploy by default. |
| **Publish target** | The distribution repository that `publish` writes to. See [Publishing](/tricks/concepts/publishing/). |
| **Catalog** | Anything `search` looks in: a skill repository, a marketplace, skills.sh, Tessl, ClawHub, GitHub code search. See [Discovery](/tricks/concepts/discovery/). |
| **Store** | A read-only, content-addressed cache of exact skill revisions, used for trials, pinned branches and snapshots. |

## Where to next

- [Install New Tricks](/tricks/getting-started/installation/).
- Walk through the [quick start](/tricks/getting-started/quick-start/) in about ten minutes.
- Use the [VS Code extension](/tricks/vscode/) if you prefer working in the editor; it runs the same core as the CLI.

Evaluations, where repo-defined test cases run headless against real agents to compare variants, are planned.
