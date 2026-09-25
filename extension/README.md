# New Tricks for VS Code

**Teach your agents new tricks.** The design-time workbench for agent skills — for Claude Code, Codex, GitHub Copilot and Cursor.

- **Discover** — one search across catalogs: skill repositories, Claude/APM marketplaces, skills.sh and GitHub, with trust, licence and risk facets. Identical copies are grouped.
- **Preview** — read any skill (and its supporting files) without installing or cloning it. Nothing is ever executed.
- **Install** — make a skill available to the agents you choose (user scope), with review-first updates and rollback.
- **Customize** — vendor an upstream skill into your source repo, edit it, and keep merging upstream improvements in VS Code's three-way merge editor.
- **Experiment** — edit on a branch, switch variants, and link a draft into any project to try it with a real agent (git status stays clean).
- **Lint** — Agent Skills spec, structure, triggering quality and safety checks in the Problems panel.
- **Publish** — one pre-flight view, then a distribution repository installable by APM, `npx skills`, Claude plugin marketplaces, Copilot, Codex and Cursor.

The extension is a thin client over the `tricks` CLI (`tricks serve --stdio`). Platform builds bundle the binary; otherwise install the CLI and make sure `tricks` is on your PATH, or set `tricks.path`.

Credentials are borrowed, never stored: `GITHUB_TOKEN`, then `gh auth token`, then your VS Code GitHub session (held in memory).
