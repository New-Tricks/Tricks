# New Tricks v0.1.0 — implementation notes

Status of the implementation of [SPEC.md](SPEC.md), what was verified and how, the decisions made while building it (for review), and known gaps.

## Layout

| Path | What |
|---|---|
| `crates/newtricks/src/` | Rust core + CLI (one crate, `lib` + `bin`). ~9k lines. |
| `crates/newtricks/tests/` | Hermetic integration tests: the real binary against local "GitHub" repositories (`TRICKS_HOST_MAP`). |
| `crates/newtricks/assets/new-tricks-skill/` | Bundled agent skill (spec §12). |
| `extension/` | VS Code extension (TypeScript, zero runtime dependencies) over `tricks serve --stdio`. |
| `.github/workflows/` | CI (Linux/macOS/Windows tests, extension build, extension-host test) and release (6 targets, checksums, platform VSIX, Marketplace/Open VSX publish). |
| `packaging/` | Homebrew formula template and release checklist. |

Core modules: `id` (grammar, URL normalization), `resolve` (refs, `@latest`, name lookup), `git` (system git, fetch-only mirrors), `sources` + `index` (adapters, FTS5, facets, dedup), `tessl` + `clawhub` (live catalog adapters), `hosted` (catalog-hosted skills: `.well-known` and ClawHub), `store` + `deploy` + `agents` (content-addressed store, links/copies, exclude handling, shadowing), `workbench` (installs, policies, updates, rollback), `workspace` + `merge` (vendoring, three-way merges, variants, worktrees), `lint`, `risk`, `license`, `publish`, `pr`, `rpc`, `statusline`, `doctor`, `selfupdate`.

## Milestones

| Milestone | Status |
|---|---|
| M1 Identity + search | Done. Adapters: git repos, `marketplace.json` (Claude + APM), skills.sh, Tessl, ClawHub and GitHub code search (live), `.well-known`, `apm.yml`/`skills-lock.json` pointer lists. |
| M2 Workbench installs | Done. Store, links/copy fallback per agent, policies, review-first updates, `auto`/`unsafe-auto` ownership rule, `--frozen`, rollback, `link`/`unlink` with exclude handling and `--shadow`, status line. |
| M3 Workspace authoring | Done. `init/vendor/import/new`, three-way merges with `--continue/--abort`, C→R candidate preview, `edit/commit/use` with worktrees and `tricks.work.toml`, lint NT1–5xx with `--fix`, bundled agent skill with commit trailers. |
| M4 Publish | Done. Gates, generated `marketplace.json` (single plugin + optional groups), metadata-only `apm.yml`, provenance, changelog, tags, `--push/--pr`, `tricks pr`. |
| M5 Distribution | Pipelines and templates done; not yet run (needs the GitHub repo, a Marketplace publisher and secrets — see `packaging/README.md`). |
| VS Code extension | Done: Discover, Preview, Workspace, Installed & Links, Problems-panel lint, diff/merge editors, publish pre-flight, status bar, frontmatter completion, `tricks.toml` schema. |

## Verification

| What | How | Result |
|---|---|---|
| Unit tests | `cargo test` (ids, URL normalization, tree hash vs git, licence detection, risk, lint rules, merge, config, framing, discovery URLs/digests/archive safety, trust levels) | 40 pass |
| M2 acceptance | `tests/workbench.rs` (scenarios 1, 4, 5 + rollback, frozen, auto ownership, upstream renames, agent skill, starred trust) | 9 pass |
| NT1xx conformance | `tests/skills_ref_conformance.rs`: all validator and parser cases ported from the official `skills-ref` (agentskills/agentskills @ 69ef37e) | 26 pass |
| `.well-known` discovery | `tests/wellknown.rs` against a local HTTP server: `skill-md`, `.tar.gz` and `.zip` entries, digest mismatch and traversal rejected, digest-based updates, unknown `$schema` refused | 2 pass |
| Tessl / ClawHub adapters | `tests/catalogs.rs` against a local HTTP server: Tessl pointer-level indexing and signals; ClawHub native search → show → add → version bump → update, tampered and unlisted files refused, `_meta.json` stripped, MIT-0 terms, re-fetch of the locked version; skills.sh mirrors and GitHub handoffs resolve to git skills | 4 pass |
| M3/M4 acceptance | `tests/workspace.rs` (scenarios 3, 6-trailer, 7 + variants, lint gate, licence override, pr dry-run) | 7 pass |
| RPC protocol | `tests/rpc.rs` (framing, dispatch, confirmation errors) | pass |
| Extension | `extension/out/test/runTest.js` in VS Code 1.128.1 (throwaway profile): activation, commands, status via real binary, lint diagnostics, virtual documents, Markdown preview | pass |
| Live search | Real GitHub + skills.sh: default sources (≈530 skills) + live adapters; cold index ≈30 s, warm online ≈6 s, offline ≈20 ms | works |
| Live Tessl / ClawHub | Real APIs, fresh sandbox, `search pdf`: 18 Tessl and 6 ClawHub listings with signals; `show`, hash-verified `add` and `outdated` of `clawhub.ai/awspace/skills//pdf`. Cold per-query cost: Tessl ≈5 s, ClawHub ≈18 s (its API takes 2–7 s per call; details are fetched in parallel); repeat queries are cached | works |
| Scenario 2 (dedup) | Live results group identical copies across catalogs | works |
| Scenario 8 (installers) | `scripts/ecosystem-test.sh`, run in CI (`ecosystem` job) and locally: publish a workspace, then `npx skills add --list`, `apm install` (into `.claude/skills/`), `claude plugin validate` + `marketplace add` + `install` (both skills loaded) | pass |
| Platforms | CI on Ubuntu, macOS and Windows (Windows runs unit, conformance and protocol tests; link-asserting integration tests are Unix-only by design) | pass |
| Quality | `cargo clippy --all-targets` and `cargo fmt --check` | clean |

## Decisions made during implementation — please review

1. **Default sources** are five skill repos (anthropics/skills, openai/skills, vercel-labs/agent-skills, github/awesome-copilot, obra/superpowers). `anthropics/claude-plugins-official` is *not* default: it points at hundreds of external repos, too many for anonymous API limits. Marketplace refreshes index at most 60 external repos.
2. **GitHub owner/repo are lowercased** in canonical IDs (GitHub is case-insensitive; catalogs disagree on casing, which broke deduplication).
3. **Slash-branch URLs** (`/tree/feature/x/…`) resolve against live `git ls-remote`; a ref missing from a stale mirror triggers one forced fetch and retry.
4. **Deployments stay on the committed version during upstream merges.** Dev links are pinned to a snapshot of `HEAD` before a merge rewrites the working tree, and return to live once the merge is committed (`tricks commit` or `install`) or aborted. This implements "deployed revisions are untouched until the merged result is committed" for dev-mode skills.
5. **Rollback pins** the skill (`update = "pinned"`) so the next check cannot undo it.
6. **`init` runs `git init`** when not inside a repository, instead of failing.
7. **`publish` exits non-zero when blocked**, including `--dry-run` (CI-friendly); the JSON report is still printed.
8. **Root `LICENSE` in the target** is copied from the workspace root if present; otherwise none is generated (skills carry their own).
9. **Untagged publishes** don't write `metadata.version`; `apm.yml` then carries the previous tag (or `0.0.0`).
10. **Publish with `--pr`** commits on `New Tricks/publish-<version>` and does not tag (tag after merging).
11. **VS Code session token** is passed as `TRICKS_VSCODE_TOKEN` and consulted *after* env vars and `gh auth token`, matching §13's order.
12. **Workspace skills deploy under their workspace name** (the `[skills.<name>]` key).
13. **Config lives in `~/.config/newtricks`** on macOS and Linux (`%APPDATA%` on Windows); data in the platform-native, non-hidden locations from §4.
14. **`install` inside a workspace** dev-links workspace skills; `-g` targets the workbench.
15. **`vendor`** sets `track = "latest"` unless a branch was given explicitly, and `update = "review"`.
16. **Lint severities**: NT203 (absolute/home paths) is a warning; NT305 (first/second-person description) was added because the spec's example config references it; unknown keys are info (NT402).
17. **Licence**: frontmatter like "Complete terms in LICENSE.txt" defers to the file; no licence found ⇒ block class; local originals are exempt from the licence gate (it applies to vendored skills, per §11).
18. **Copilot is always copy-mode**: one `copilot` agent serves both VS Code (symlink bug) and the CLI through `~/.copilot/skills`.
19. **Name and placeholders**: renamed from *skillbench* to **New Tricks** (command `tricks`) because "SkillBench" is an existing brand shipping agent-skill marketplaces. GitHub: org `new-tricks` (lowercase), repo `new-tricks/tricks`, tap `new-tricks/homebrew-tap`. The extension publisher `newtricks` is still to be registered.
20. **`New Tricks:` URIs** encode the skill ID as base64url (IDs contain `//`, which broke parse/serialize round trips).
21. **Search refreshes stale sources on demand** (24 h default); the first search on a new machine is the slow one.
22. **`tricks statusline`** is the agent status-line integration (cache-only; forced offline).
23. **All git access shells out to the system `git`**, reads included; no embedded git library (§4 allowed one for fast reads). Simpler, one git behaviour everywhere; performance has been adequate.
24. **Concurrency** uses `std` file locks (Rust ≥ 1.89): one per upstream source around clone/fetch, one per workspace around merges and publishes, plus SQLite WAL with a busy timeout. This covers the CLI and the extension's server running at the same time.
25. **ClawHub search excludes skills ClawHub flags as suspicious** (`nonSuspiciousOnly=true`). They can still be installed by explicit ID (no catalog signals are shown then, since they come from search listings).
26. **ClawHub-native IDs are `clawhub.ai/<owner>/skills//<slug>`**, mirroring ClawHub's own URLs so a pasted `https://clawhub.ai/<owner>/skills/<slug>` normalizes to the ID. The lock records `clawhub:<version>` as the commit.
27. **ClawHub downloads are verified file by file** against the version's published SHA-256 list: mismatched, missing or unlisted files abort the install, and a version with no published hashes is refused. ClawHub's `_meta.json` (registry bookkeeping, not in the hash list) is stripped.
28. **ClawHub licence = MIT-0 unless the skill says otherwise** (ClawHub's publishing terms), as a last-resort source after the skill's own licence. Re-uploads of proprietary skills (e.g. copies of Anthropic's `pdf`) stay blocked by their frontmatter.
29. **VirusTotal "suspicious" is not a risk flag** (it fires on any shell usage, even when ClawHub's overall status is clean); `malicious`, ClawHub's suspicious/malware flags and non-clean moderation or security status are.
30. **GitHub-backed ClawHub skills install as git skills**: the download handoff is redirected to `github.com/<repo>//<path>`, tracking the repository normally rather than ClawHub's scanned commit. They are skipped in search results (no published version to index).
31. **Tessl uses `/experimental/search`**, its only public search endpoint. Only the pointed-to skill directories are indexed (per-directory freshness), since Tessl often points into large application repositories.
32. **`[settings] live`** selects the live-query adapters (default: skills.sh, Tessl, ClawHub, GitHub); `--no-live` skips all of them for one search.

## Known gaps

- **Not exercised against live GitHub writes:** `tricks pr` (non-dry-run: creates a public fork and PR), `publish --push/--pr`, `self-update` (no release exists yet). Dry-run and local paths are tested.
- **Windows with real agents installed**: CI passes on Windows runners, but copy-mode placement hasn't been checked against installed Claude Code/Codex/Copilot/Cursor on Windows.
- **Cursor hidden-directory check (§17)** not run: needs an authenticated Cursor agent. The Linux rule is coded conservatively (copy when the target path is hidden).
- **Extension UI** (Discover webview, publish panel, merge editor wiring) is covered by activation/RPC tests but not by UI automation.
- **macOS signing/notarization** step is a placeholder in `release.yml`.
- **Cold live search is network-bound**: the live adapters run one after another, so a first-time query costs ≈30 s with all four enabled (ClawHub's API is the slowest); repeats are cached per query. Running the adapters' network phases concurrently is the next step if this matters in practice.
- **Deliberate differences from `skills-ref`**: unknown frontmatter keys are info (NT402) unless `strict-spec` is set, and `skill.md` is accepted with a warning (NT110) because Claude Code only loads `SKILL.md`.
