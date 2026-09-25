# New Tricks v0.3.0 — implementation notes

Status of the implementation of [SPEC.md](SPEC.md), what was verified and how, the decisions made while building it (for review), and known gaps.

## Layout

| Path | What |
|---|---|
| `crates/newtricks/src/` | Rust core + CLI (one crate, `lib` + `bin`). ~9k lines. |
| `crates/newtricks/tests/` | Hermetic integration tests: the real binary against local "GitHub" repositories (`TRICKS_HOST_MAP`). |
| `skills/new-tricks/` | Bundled agent skill (spec §12); also installable with `npx skills add new-tricks/tricks`. |
| `extension/` | VS Code extension (TypeScript, zero runtime dependencies) over `tricks serve --stdio`. |
| `.github/workflows/` | CI (Linux/macOS/Windows tests, extension build, extension-host test) and release (6 targets, checksums, platform VSIX, Marketplace/Open VSX publish). |
| `packaging/` | Homebrew formula template and release checklist. |

Core modules: `id` (grammar, URL normalization), `resolve` (refs, `@latest`, name lookup), `git` (system git, fetch-only mirrors), `catalogs` + `index` (adapters, FTS5, facets, dedup), `tessl` + `clawhub` (live catalog adapters), `hosted` (catalog-hosted skills: `.well-known` and ClawHub), `live` (concurrent live-query adapters), `store` + `deploy` + `agents` (content-addressed store cache, placements/copies, exclude handling, shadowing), `links` (link/unlink of repo skills and trials), `user` (user config), `source_repo` + `merge` (vendoring, three-way merges for git and catalog-hosted upstreams, variants, worktrees), `lint`, `risk`, `license`, `publish`, `contribute`, `rpc`, `statusline`, `doctor`, `selfupdate`.

## Milestones

| Milestone | Status |
|---|---|
| M1 Identity + search | Done. Adapters: git repos, `marketplace.json` (Claude + APM), skills.sh, Tessl, ClawHub and GitHub code search (live), `.well-known`, `apm.yml`/`skills-lock.json` pointer lists. |
| M2 Links | Done. Store cache, links/copy fallback per agent, `link`/`unlink` of source repo skills (dev mode, variants) and upstream trials, exclude handling and `--shadow`, status line. (User-scope installs, policies and rollback existed in 0.1–0.2 and were removed in 0.3.) |
| M3 Source repo authoring | Done. `init/vendor/new` (vendor takes upstream git or catalog-hosted skills and local folders), three-way `merge` with `--dry-run/--continue/--abort` for git and catalog-hosted upstreams, C→R candidate preview, `edit [--branch|--done]/use` with worktrees and `tricks.work.toml`, lint NT1–5xx with `--fix`, bundled agent skill. |
| M4 Publish | Done. Gates, generated `marketplace.json` (single plugin + optional groups), metadata-only `apm.yml`, provenance, changelog, tags, `--push/--pr`, `tricks contribute`. |
| M5 Distribution | Done: releases publish binaries, platform VSIX to the VS Code Marketplace and Open VSX, and the Homebrew formula (see `packaging/README.md`). |
| VS Code extension | Done: Discover (Try, Vendor), Preview, Source Repo, Links, Problems-panel lint, diff/merge editors, publish pre-flight, status bar, frontmatter completion, `tricks.toml` schema. |

## Verification

| What | How | Result |
|---|---|---|
| Unit tests | `cargo test` (ids, URL normalization, tree hash vs git, licence detection, risk, lint rules, merge, config, framing, discovery URLs/digests/archive safety, trust levels) | 41 pass |
| Links | `tests/links.rs` (scenarios 1, 5 + trials into the current project, repo skills linked globally in dev mode and unlinked together, worktree exclude sharing, 0.2 user-scope installs reported and removed, starred trust) | 7 pass |
| NT1xx conformance | `tests/skills_ref_conformance.rs`: all validator and parser cases ported from the official `skills-ref` (agentskills/agentskills @ 69ef37e) | 26 pass |
| `.well-known` discovery | `tests/wellknown.rs` against a local HTTP server: `skill-md`, `.tar.gz` and `.zip` entries, digest mismatch and traversal rejected, trials, vendoring and digest-based merges, unknown `$schema` refused | 2 pass |
| Tessl / ClawHub adapters | `tests/catalogs.rs` against a local HTTP server: Tessl pointer-level indexing and signals; ClawHub native search → show → trial link → vendor → customize → version bump → merge, tampered and unlisted files refused, `_meta.json` stripped, MIT-0 terms, base snapshot re-fetched by version; skills.sh mirrors and GitHub handoffs resolve to git skills (link and vendor); skills.sh, Tessl and ClawHub queried together all list the same skill, and repeats are served from the cache | 5 pass |
| M3/M4 acceptance | `tests/source_repo.rs` (scenarios 3, 4, 7 + `merge --dry-run`, merges committed with git, variants and `edit --done`, `vendor <folder> --upstream --base`, upstream renames followed, lint gate, licence override from the manifest, contribute dry-run, init agent skill at project scope) | 9 pass |
| RPC protocol | `tests/rpc.rs` (framing, dispatch, trial link, vendor confirmation errors, merge dry run, status) | pass |
| Extension | `extension/out/test/runTest.js` in VS Code 1.128.1 (throwaway profile): activation, commands, status via real binary, linking and unlinking the source repo's skills, lint diagnostics, virtual documents, Markdown preview; Discover messages through the real core (search, copy, `tricks.search` handing the view its query); publishing from the pre-flight panel through the core's confirmation pushed to a bare target remote (files, tag) | pass |
| Extension webviews | `extension/out/test/webview.test.js` (`npm test`): the real Discover and publish pages and scripts in jsdom — initial and debounced searches, facets, Enter, remembered state, card rendering and tags, card actions, messages from the extension; gates, preselected bump, publish options, blocked state; skill and report text never becomes markup or script | pass |
| Live search | Real GitHub + skills.sh: default catalogs (≈530 skills) + live adapters; cold index ≈30 s, warm online ≈6 s, offline ≈20 ms | works |
| Live Tessl / ClawHub | Real APIs, fresh sandbox, `search pdf`: 18 Tessl and 6 ClawHub listings with signals; `show` and a hash-verified download of `clawhub.ai/awspace/skills//pdf`. Live adapters run concurrently (`src/live.rs`): cold `search pdf` with all four ≈9 s, `react testing` ≈15 s (was ≈21–30 s sequentially), bounded by the slowest catalog or repository fetch; repeat queries are cached | works |
| Scenario 2 (dedup) | Live results group identical copies across catalogs | works |
| Scenario 8 (installers) | `scripts/ecosystem-test.sh`, run in CI (`ecosystem` job) and locally: publish a source repo, then `npx skills add --list`, `apm install` (into `.claude/skills/`), `claude plugin validate` + `marketplace add` + `install` (both skills loaded) | pass |
| Platforms | CI on Ubuntu, macOS and Windows (Windows runs unit, conformance and protocol tests; link-asserting integration tests are Unix-only by design) | pass |
| Quality | `cargo clippy --all-targets` and `cargo fmt --check` | clean |

## Decisions made during implementation — please review

1. **Default catalogs** are five skill repos (anthropics/skills, openai/skills, vercel-labs/agent-skills, github/awesome-copilot, obra/superpowers). `anthropics/claude-plugins-official` is *not* default: it points at hundreds of external repos, too many for anonymous API limits. Marketplace refreshes index at most 60 external repos.
2. **GitHub owner/repo are lowercased** in canonical IDs (GitHub is case-insensitive; catalogs disagree on casing, which broke deduplication).
3. **Slash-branch URLs** (`/tree/feature/x/…`) resolve against live `git ls-remote`; a ref missing from a stale mirror triggers one forced fetch and retry.
4. **Deployments stay on the committed version during upstream merges.** Dev links are pinned to a snapshot of `HEAD` before a merge rewrites the working tree, and return to live once the merge is committed (with `git commit`; the next `tricks` command in the repo re-points them) or aborted. This implements "deployed revisions are untouched until the merged result is committed" for dev-mode skills.
5. **Vendored skills' `update` policy** is `review`, `pinned` (skipped by `merge` unless named) or `paused` (not checked); `auto`/`unsafe-auto` went with user scope.
6. **`init` runs `git init`** when not inside a repository, instead of failing.
7. **`publish` exits non-zero when blocked**, including `--dry-run` (CI-friendly); the JSON report is still printed.
8. **Root `LICENSE` in the target** is copied from the source repo root if present; otherwise none is generated (skills carry their own).
9. **Untagged publishes** don't write `metadata.version`; `apm.yml` then carries the previous tag (or `0.0.0`).
10. **Publish with `--pr`** commits on `New Tricks/publish-<version>` and does not tag (tag after merging).
11. **VS Code session token** is passed as `TRICKS_VSCODE_TOKEN` and consulted *after* env vars and `gh auth token`, matching §13's order.
12. **Source repo skills deploy under their name in the source repo** (the `[skills.<name>]` key).
13. **Config lives in `~/.config/newtricks`** on macOS and Linux (`%APPDATA%` on Windows); data in the platform-native, non-hidden locations from §4.
14. **`link` with no skill** inside a source repo links all of its skills (dev mode, user-level agent directories); `unlink` with no skill removes them.
15. **`vendor`** sets `track = "latest"` unless a branch was given explicitly, and `update = "review"`.
16. **Lint severities**: NT203 (absolute/home paths) is a warning; NT305 (first/second-person description) was added because the spec's example config references it; unknown keys are info (NT402).
17. **Licence**: frontmatter like "Complete terms in LICENSE.txt" defers to the file; no licence found ⇒ block class; local originals are exempt from the licence gate (it applies to vendored skills, per §11).
18. **Copilot is always copy-mode**: one `copilot` agent serves both VS Code (symlink bug) and the CLI through `~/.copilot/skills`.
19. **Name and placeholders**: renamed from *skillbench* to **New Tricks** (command `tricks`) because "SkillBench" is an existing brand shipping agent-skill marketplaces. GitHub: org `new-tricks` (lowercase), repo `new-tricks/tricks`, tap `new-tricks/homebrew-tap`. The extension publisher `newtricks` is still to be registered.
20. **`New Tricks:` URIs** encode the skill ID as base64url (IDs contain `//`, which broke parse/serialize round trips).
21. **Search refreshes stale catalogs on demand** (24 h default); the first search on a new machine is the slow one.
22. **`tricks statusline`** is the agent status-line integration (cache-only; forced offline).
23. **All git access shells out to the system `git`**, reads included; no embedded git library (§4 allowed one for fast reads). Simpler, one git behaviour everywhere; performance has been adequate.
24. **Concurrency** uses `std` file locks (Rust ≥ 1.89): one per upstream repository around clone/fetch, one per source repo around merges and publishes, one per publish target around its clone, plus SQLite WAL with a busy timeout. This covers the CLI and the extension's server running at the same time.
25. **ClawHub search excludes skills ClawHub flags as suspicious** (`nonSuspiciousOnly=true`). They can still be linked or vendored by explicit ID (no catalog signals are shown then, since they come from search listings).
26. **ClawHub-native IDs are `clawhub.ai/<owner>/skills//<slug>`**, mirroring ClawHub's own URLs so a pasted `https://clawhub.ai/<owner>/skills/<slug>` normalizes to the ID. Vendored ones record `clawhub:<version>` as their base.
27. **ClawHub downloads are verified file by file** against the version's published SHA-256 list: mismatched, missing or unlisted files abort the link, vendor or merge, and a version with no published hashes is refused. ClawHub's `_meta.json` (registry bookkeeping, not in the hash list) is stripped.
28. **ClawHub licence = MIT-0 unless the skill says otherwise** (ClawHub's publishing terms), as a last-resort source after the skill's own licence. Re-uploads of proprietary skills (e.g. copies of Anthropic's `pdf`) stay blocked by their frontmatter.
29. **VirusTotal "suspicious" is not a risk flag** (it fires on any shell usage, even when ClawHub's overall status is clean); `malicious`, ClawHub's suspicious/malware flags and non-clean moderation or security status are.
30. **GitHub-backed ClawHub skills are linked and vendored as git skills**: the download handoff is redirected to `github.com/<repo>//<path>`, tracking the repository normally rather than ClawHub's scanned commit. They are skipped in search results (no published version to index).
31. **Tessl uses `/experimental/search`**, its only public search endpoint. Only the pointed-to skill directories are indexed (per-directory freshness), since Tessl often points into large application repositories.
32. **`[settings] live`** selects the live-query adapters (default: skills.sh, Tessl, ClawHub, GitHub); `--no-live` skips all of them for one search.
33. **Live adapters run concurrently.** Each adapter's network work — the catalog query, ClawHub details, and fetching the GitHub repositories and directories it points at — runs on its own thread; the database thread stores each batch as it arrives. A claim set stops two catalogs fetching the same repository twice, and listings whose skill arrives in another catalog's later batch are retried at the end. Sources not indexed through the GitHub API (other hosts, `TRICKS_NO_API`) are still indexed on the database thread.
34. **One scope (0.3): the source repo.** User-scope installs, update policies, rollback, the user lock and their state tables are removed; APM, `npx skills` and plugin marketplaces own workstation installs. Removed commands fail with clap's "similar commands" hint; there are no compatibility aliases (only the visible aliases `info` and `ls`).
35. **Link defaults**: source repo skills and local folders link into the user-level agent directories, upstream skills (trials) into the current project; `--to`/`--global` override. Repo skill links (any scope) follow `edit`, `use` and merges; trials are fixed revisions, never locked or updated.
36. **The store is a cache**: `unlink` prunes what no placement needs; kept roots are placements plus the base and last-fetched snapshots of catalog-hosted upstreams. `gc` stays as hidden plumbing.
37. **Catalog-hosted upstreams can be vendored and merged.** Their base snapshot is kept in the store; a missing ClawHub base is fetched again by version and verified; a missing `.well-known` base cannot be (those indexes serve only the latest) and asks for a re-vendor. `merge` re-reads a stale `.well-known` index first so new digests are seen.
38. **`merge --dry-run` replaces `outdated`** (it fetches and reports incoming changes and risk); `status` reports upstream changes from caches only.
39. **`edit --done` replaces `commit`**: drafts are committed with plain git (the agent skill adds a `Tricks-Agent:` trailer). Merge snapshots are released by the next `tricks` command after the merge is committed (`status`, `link`, …; the extension's status refresh does it too).
40. **`pr` is `contribute`**, to stop clashing with `publish --pr`. **`allow-license` is a manifest key** (`license-override = { justification = "…" }`) that the blocked licence gate prints.
41. **The bundled agent skill** lives at `skills/new-tricks/`: `init --agent-skill` installs it into a source repo; `npx skills add new-tricks/tricks` installs it everywhere. The first-run offer and `agent-skill` command are gone.
42. **0.2 user-scope installs** keep working (their store entries stay referenced); `doctor` and `status` report them and `unlink --legacy` removes them.
43. **`agents` and `source-repos` folded** into `doctor` (agent directories, modes, tested versions) and `status` (registered repos outside a repo).

## Known gaps

- **Not exercised against live GitHub writes:** `tricks contribute` (non-dry-run: creates a public fork and PR), `publish --push/--pr`, `self-update` (not yet run against a real release). Dry-run and local paths are tested.
- **Windows with real agents installed**: CI passes on Windows runners, but copy-mode placement hasn't been checked against installed Claude Code/Codex/Copilot/Cursor on Windows.
- **Cursor hidden-directory check (§17)** not run: needs an authenticated Cursor agent. The Linux rule is coded conservatively (copy when the target path is hidden).
- **Extension UI**: the Discover and publish webviews are tested in jsdom and through their message handlers in the extension host, not by driving VS Code's own UI (no WebdriverIO/ExTester). The merge-editor wiring is only covered by activation tests.
- **macOS signing/notarization** step is a placeholder in `release.yml`.
- **Deliberate differences from `skills-ref`**: unknown frontmatter keys are info (NT402) unless `strict-spec` is set, and `skill.md` is accepted with a warning (NT110) because Claude Code only loads `SKILL.md`.
