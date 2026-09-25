---
title: Agents
description: Which coding agents New Tricks links skills for, where each one looks for skills, and when a link becomes a copy.
---

When you [link or try](/tricks/concepts/links-and-trials/) a skill, New Tricks places it in the primary skill directory of each agent you select: the user-level directory by default for `link`, the project's directory for `try` and `--to`. It supports four agents.

## Where skills go

| Agent | ID | User-level directory | Project directory | Also reads (user level) | Follows links |
|---|---|---|---|---|---|
| Claude Code | `claude` | `~/.claude/skills/` | `.claude/skills/` | Nothing else (it doesn't read `.agents/skills`) | Yes |
| Codex | `codex` | `~/.agents/skills/` | `.agents/skills/` | `~/.codex/skills/` (deprecated), `/etc/codex/skills` | Yes, in user, repo and admin scope |
| Cursor | `cursor` | `~/.cursor/skills/` | `.cursor/skills/` | `~/.agents/skills/`, `~/.claude/skills/`, `~/.codex/skills/` | Yes, but it skips hidden directories |
| GitHub Copilot | `copilot` | `~/.copilot/skills/` | `.github/skills/` | `~/.claude/skills/`, `~/.agents/skills/` | The CLI does. In VS Code, linked skills are listed but the `skill()` tool fails on them ([microsoft/vscode#315979](https://github.com/microsoft/vscode/issues/315979)) |

Claude Code and Codex look for project skills from the current directory up to the repository root. Cursor searches project directories recursively.

New Tricks places a skill only in the primary directory of each agent you select. It doesn't try to stop other agents from seeing it: because Cursor and Copilot also read `~/.claude/skills/`, a user-level link for Claude Code shows up in them too. That's usually what you want when testing. When the same skill reaches an agent through more than one directory, the agent may show it twice. Copilot is known to list skills twice when directories link to each other.

The `copilot` integration serves both Copilot in VS Code and the Copilot CLI.

## Choose the agents

`link` and `try` take `--agents` with a comma-separated list of IDs, or `all`:

```bash
tricks link changelog-writer --to ~/code/my-app --agents claude,copilot,cursor
```

```text
warning: some selected agents cannot follow links here; they get a copy, so live edits will not show until you re-link
linked changelog-writer into ~/code/my-app from main (working tree, live)
  claude   ~/code/my-app/.claude/skills/changelog-writer (link)
  copilot  ~/code/my-app/.github/skills/changelog-writer (copy)
  cursor   ~/code/my-app/.cursor/skills/changelog-writer (link)
```

A few aliases are accepted too: `claude-code`, `github-copilot`, `copilot-cli`, `vscode`, `cursor-agent` and `openai-codex`.

Without `--agents`, New Tricks uses:

1. The source repo's `agents`, when you run the command inside a source repo that sets it:

   ```toml
   # tricks.toml
   [source-repo]
   agents = ["claude", "codex"]
   ```

2. Otherwise, the `agents` setting in your user config (`~/.config/newtricks/tricks.toml`), which is `["claude"]` by default:

   ```toml
   [settings]
   agents = ["claude"]      # agents to link skills for (a source repo can set its own)
   ```

See [Configuration](/tricks/reference/configuration/) for both files.

## Link or copy

Each agent declares whether it follows directory links on each platform. Where it doesn't, that agent gets a copy, and the store remains the source of truth.

| Agent | macOS | Linux | Windows |
|---|---|---|---|
| Claude Code | link | link | copy |
| Codex | link | link | copy |
| Cursor | link | link, or copy when the target is under a hidden directory | copy |
| GitHub Copilot | copy | copy | copy |

- **Windows** uses copies for every agent, because linked skill directories fail to load there (for example [anthropics/claude-code#41177](https://github.com/anthropics/claude-code/issues/41177)).
- **Copilot** always gets a copy until microsoft/vscode#315979 is fixed.
- **Cursor on Linux** skips hidden dot-directories during discovery. When any directory in the link's target path starts with `.`, Cursor gets a copy instead. That includes the store under `~/.local/share/newtricks/` and drafts under `.tricks/work/`. On macOS and Windows the data directory isn't hidden.

A copy of a live target, such as your working tree or a draft, doesn't follow your edits. Run `tricks link` again to refresh it. You can also force copies for every agent with `--copy`.

### Override with `TRICKS_LINK_MODE`

The `TRICKS_LINK_MODE` environment variable overrides these defaults, for testing or when an upstream fix ships before New Tricks catches up:

```bash
TRICKS_LINK_MODE=copy tricks link                        # copies for every agent
TRICKS_LINK_MODE=link tricks link                        # links for every agent
TRICKS_LINK_MODE=copilot=link,claude=copy tricks link    # per agent; others keep their default
```

Per-agent entries use the agent IDs from the table above.

## Check your setup with `tricks doctor`

[`tricks doctor`](/tricks/reference/commands/doctor/) reports each agent's directories, whether the user-level directory exists yet, the mode links into the store get on this machine, and the agent version the integration was tested against:

```text
New Tricks 0.6.0
  ✓ git                      git version 2.55.0
  ✓ gh                       GitHub CLI found
  ✗ github.com credentials   anonymous: public sources only, 60 API requests/hour, no GitHub code search. Run `gh auth login`.
  ✓ source repo my-skills    ~/code/my-skills
  ✓ config                   ~/.config/newtricks
  ✓ data                     ~/Library/Application Support/newtricks
  ✓ store                    0 revision(s)
  ✓ index                    0 skill(s) indexed
  ✓ agent claude             user ~/.claude/skills (exists), project .claude/skills; link mode; tested 2.1.280
  ✓ agent codex              user ~/.agents/skills (not created yet), project .agents/skills; link mode; tested 0.140.0
  ✓ agent cursor             user ~/.cursor/skills (not created yet), project .cursor/skills; link mode; tested 3.11.19
  ✓ agent copilot            user ~/.copilot/skills (not created yet), project .github/skills; copy mode; tested VS Code 1.128.1
  ✓ operations               no interrupted operations
  ✓ placements               all healthy
```

The `placements` line lists every link that isn't healthy, with its state. See [Link health](/tricks/concepts/links-and-trials/#link-health). With `TRICKS_LINK_MODE` set, the agent lines show the overridden mode:

```text
  ✓ agent claude             user ~/.claude/skills (exists), project .claude/skills; copy mode; tested 2.1.280
  ✓ agent copilot            user ~/.copilot/skills (not created yet), project .github/skills; link mode; tested VS Code 1.128.1
```
