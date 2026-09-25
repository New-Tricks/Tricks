# New Tricks — v1 specification

Status: design agreed; implemented (0.6.1) — see [IMPLEMENTATION.md](IMPLEMENTATION.md) for status, verification, implementation decisions and known gaps.

Updated: 25 September 2026 (0.3: New Tricks works on source repos only; workstation installs removed). Supersedes *Skills Manager — first-version specification* (17 September 2026).

## 1. Positioning

**New Tricks is the design-time workbench for agent skills.** It operates on one construct, the **source repo**: a git repository holding draft skills, customized copies of upstream skills, and their scripts, evals and tests. New Tricks finds prior art across fragmented catalogs, keeps vendored skills merging upstream improvements, links skills into agent directories so you can validate them with real agents, lints them, and publishes a clean, installable distribution repository.

It deliberately does **not** manage the skills installed on a workstation. Projects commit an `apm.yml` and install with [APM](https://github.com/microsoft/apm); anyone can install published skills with `npx skills`, a Claude plugin marketplace, or by copying the directory. New Tricks is the tool you use *before* that point, and the one that makes a skills repository consumable by all of them at once.

| Stage | Tool |
|---|---|
| Discover prior art, preview, trial | **New Tricks** (`search`, `info`, `view`, `try`) |
| Author, vendor and customize, experiment, validate, lint | **New Tricks** (source repo) |
| Publish a distribution repository | **tricks publish** |
| Install into workstations, projects, CI | APM, `npx skills`, plugin marketplaces |

Differentiators no current tool combines:

1. Federated search over many skill indexes with a single normalized model, deduplicated by content.
2. Vendoring upstream skills into your own repository with a recorded base, so upstream changes three-way-merge into your customizations.
3. Branch-based experiments and test deployments into any project without polluting its git state.
4. One publish producing a repository installable by APM, `npx skills`, Claude plugin marketplaces, Copilot, Codex and Cursor.

## 2. Scope

### In v1

- CLI on macOS, Linux and Windows; full functionality without a GUI.
- VS Code extension (also Cursor, Windsurf, VSCodium via Open VSX) as the only GUI.
- Agents: **Claude Code, Codex, GitHub Copilot, Cursor**.
- Federated search, preview, and trials: link an upstream skill into a project without vendoring it.
- Source repo authoring: vendor (upstream git or catalog-hosted skills, or local folders), new, lint, upstream merge, branch experiments, links of repo skills into agent directories for testing.
- Publishing with gates, generated ecosystem manifests, versioning and changelog.
- Bundled agent skill so agents can use New Tricks safely.

### Explicitly out of v1

| Item | Status |
|---|---|
| Evaluations / test harness | Next (`tricks test`): repo-defined cases run headless against real agents, comparing variants. |
| Per-version success metrics | With `tricks test` (pass rate, trigger rate, tokens, time per variant). |
| Workstation skill management (installs, updates, pinning, rollback) | Dropped in 0.3. Owned by APM, `npx skills` and plugin marketplaces; source repos publish to them. |
| Agent × skill assignment matrix | Dropped. Skills are linked for the selected agents; other agents reading shared directories is an accepted side effect. |
| Adopting existing local installations | Dropped. Skills enter via search or `vendor <folder>`. |
| Native desktop apps (Swift / GTK / WinUI) | Shelved in favour of the VS Code extension. |
| Background daemon or OS scheduler | Dropped. Freshness checks run when New Tricks is invoked. |
| Managing remote repositories (private copies) | Dropped. The source repo *is* the customized copy; the user owns its remote. |
| Project-level dependency management | Owned by APM. New Tricks does not replace `apm.yml`. |
| Semantic / embedding search | Deferred; adapter model allows it later. |
| LobeHub and long-tail directories | Deferred (client registration required, or no API / terms-of-service questions). |
| Codex plugin marketplace file (`.agents/plugins/marketplace.json`) | Deferred; Codex installs the bare `skills/` layout already. |
| Publish-time templating / transforms beyond excludes | Deferred. |
| Source repo hosting other than GitHub for PR flows | Deferred; indexing and install from other git hosts work read-only. |

## 3. Terminology

| Term | Meaning |
|---|---|
| **Source repo** | A git repository where skills are authored, customized, tested and published from. Contains `tricks.toml` and `tricks.lock`. The user owns its remote and pushes. The one thing New Tricks operates on. |
| **User config** | Machine-level settings, catalogs and registered source repos, at `~/.config/newtricks/tricks.toml`. |
| **Upstream** | The repository and path a vendored skill came from. |
| **Vendored skill** | A copy of an upstream skill inside a source repo, with its upstream and base commit recorded. |
| **Local original** | A skill authored in (or vendored from a local folder into) a source repo with no upstream. |
| **Catalog** | Anything New Tricks searches: a skill repository, or an index that points at skills in repositories (skills.sh, `marketplace.json`, APM marketplace, Tessl, ClawHub, GitHub search). Managed with `tricks catalog`. |
| **Store** | Immutable, content-addressed cache of exact skill revisions (variants, trials, merge snapshots, catalog-hosted bases). |
| **Link** | A symlink (junction or copy where needed) from an agent skill directory to a source repo skill (dev mode or a variant) or to an upstream skill under trial. |
| **Trial** | A link of an upstream skill that is not in the source repo: evaluation only, never locked or updated. |
| **Target (link)** | A scope — the user-level agent directories (`--global`) or a project path — plus a set of agents. |
| **Publish target** | The distribution repository that receives the published skills, named by its remote (`owner/repo`, a git URL, or a path). New Tricks publishes through its own clone of it. |
| **B / C / U / R** | B: upstream revision last incorporated. C: the source repo's customized version. U: latest fetched upstream. R: candidate merge of C with U. |

## 4. Architecture

```
┌─────────────────────┐   JSON-RPC over stdio   ┌───────────────────────────────┐
│ VS Code extension   │ ──────────────────────▶ │ tricks serve --stdio      │
│ (TypeScript, no     │                         │                               │
│  business logic)    │                         │  New Tricks core (Rust crate) │
└─────────────────────┘                         │  ─ identity & resolution      │
┌─────────────────────┐   in-process            │  ─ catalog adapters & index   │
│ New Tricks CLI      │ ──────────────────────▶ │  ─ store & links              │
└─────────────────────┘                         │  ─ source repo & merge        │
                                                │  ─ lint & risk scan           │
                                                │  ─ publish                    │
                                                └──────────────┬────────────────┘
                                                               │
                                     system git (writes)  ·  embedded git (reads)
                                     SQLite state.db      ·  GitHub API (borrowed token)
```

- **Rust core library** used by the CLI binary. The VS Code extension bundles the platform binary and runs `tricks serve --stdio` as a child process for its lifetime (the ruff / biome / rust-analyzer model). No daemon, no OS scheduler.
- **System `git`** performs all writes (fetch, merge, commit, push), inheriting the user's credential helpers and matching what users see with their own git tools. An embedded git library is used only for fast read-only operations (trees, history, hashing).
- **Three-way merges** use git's merge machinery (`git merge-file` / `git merge-tree`).
- **Concurrency**: per-upstream, per-source-repo and per-publish-target file locks plus an operation journal in `state.db`; interrupted operations are recoverable and report a specific retry action.

### On-disk layout

```
~/.config/newtricks/                # %APPDATA%\newtricks on Windows
  tricks.toml                    # user config: settings, catalogs, registered source repos
<data-dir>/                          # see table below
  store/<tree-hash>/                 # immutable, read-only skill revisions (a cache)
  repos/<host>/<owner>/<repo>/       # fetch-only upstream mirrors (never pushed)
  publish/<key>/                     # New Tricks' clones of publish targets
  backups/                           # originals displaced by --shadow
  state.db                           # index (FTS5), catalog, links, merges, journal
```

The data directory uses platform-native, non-hidden locations where the platform allows, because Cursor skips hidden dot-directories during discovery:

| Platform | `<data-dir>` |
|---|---|
| macOS | `~/Library/Application Support/newtricks/` |
| Windows | `%LOCALAPPDATA%\newtricks\` |
| Linux | `$XDG_DATA_HOME/newtricks/` (default `~/.local/share/newtricks/`, hidden — see §8 and §17) |

## 5. Skill identity

Identity is **host + repository + repository-relative path**. Frontmatter `name` is a lookup key and display name, never identity (names are not unique across repositories).

### Grammar

```
skill-ref  ::= [ host "/" ] owner "/" repo "//" path-or-name [ "@" ref ]
source-ref ::= [ host "/" ] owner "/" repo
```

- **`//` is mandatory** in every skill reference. It marks where the repository ends and the in-repo path begins (Terraform / go-getter convention), which keeps IDs unambiguous on hosts with nested groups. A reference without `//` denotes a repository, not a skill.
- **Host**: if the first segment contains a dot it is a host (e.g. `github.mit.edu`); otherwise `github.com`.
- **After `//`**: resolved as an exact path first (`//skills/pdf`), then by frontmatter `name` within the repository (`//pdf`), then by folder name. Must resolve uniquely; otherwise error listing candidates. A repository whose root is a skill is referenced by its name and stored canonically as `//.`.
- **Canonical form** — always fully expanded host, `//`, full path, and resolved ref — is used in every file, lockfile and `--json` output.

### Refs (Go style)

| Form | Meaning |
|---|---|
| *(none)* | `@latest` |
| `@latest` | Highest semver tag; if the repository has no tags, the default branch head (read from the repository, not assumed to be `main`) |
| `@v1.3.0` | Tag |
| `@main`, `@feature/terse` | Branch (slashes allowed; the ref is everything after the last `@`) |
| `@3f2a1c9` | Commit (prefix ≥ 7) |
| `@refs/heads/x`, `@refs/tags/x` | Explicit full ref, to disambiguate |

Ambiguous short names resolve tag → branch → commit, with a warning when a tag and branch share a name. The lockfile always pins the resolved commit and tree hash.

### Examples

```
anthropics/skills//pdf                      → github.com/anthropics/skills//skills/pdf@v1.4.0
github.mit.edu/ist-org/skills//docx@v1.0.0  → github.mit.edu/ist-org/skills//skills/docx@v1.0.0
```

### URL normalization

Every GitHub URL form normalizes to canonical:

| Input | Canonical |
|---|---|
| `https://github.com/anthropics/skills` | `github.com/anthropics/skills` |
| `https://github.com/anthropics/skills/tree/main/skills/pdf` | `github.com/anthropics/skills//skills/pdf@main` |
| `https://github.com/anthropics/skills/blob/v1.3.0/skills/pdf/SKILL.md` | `github.com/anthropics/skills//skills/pdf@v1.3.0` |
| `git@github.com:anthropics/skills.git`, `https://…/skills.git` | `github.com/anthropics/skills` |

`/tree/<a>/<b>/…` is ambiguous when branch names contain slashes; resolve by matching the longest prefix that exists as a ref (API or `git ls-remote`).

### Catalog-hosted skills

Some catalogs host skills outside git. They use the same grammar, with the catalog as host:

| Catalog | ID | Ref | Lock `commit` |
|---|---|---|---|
| `.well-known/agent-skills` | `example.com/.well-known/agent-skills//<name>` | — (the index digest) | `sha256:<digest>` |
| ClawHub native | `clawhub.ai/<owner>/skills//<slug>` (mirrors ClawHub's `/<owner>/skills/<slug>` URLs, which normalize to it) | `@<version>` | `clawhub:<version>` |

Vendored from a catalog, such a skill records the catalog id as its upstream and the digest or version as its base; the base snapshot stays in the store (ClawHub versions can also be fetched again), and `merge` compares the latest published revision with it (§10). A ClawHub skill that ClawHub itself serves from GitHub (no published version; the download is a GitHub handoff) is linked or vendored as the git skill it points to.

### Content identity

- **Version truth**: commit SHA. Tags and branches are labels.
- **Content hash**: git tree hash of the skill directory. Used for store addressing, integrity verification, and grouping identical copies across repositories and catalogs.
- **Upstream renames/moves** are detected with git rename tracking during updates and recorded as aliases.

## 6. Manifest and lock files

Conventions follow Cargo (TOML; intent in the manifest, resolved facts in the lock) with Go semantics (path identity, `@ref`, pseudo-versions for untagged commits). Filenames are tool-specific to avoid collisions: `tricks.toml`, `tricks.lock`, `tricks.work.toml`. Regular projects never contain these files — they commit `apm.yml`.

### User config

```toml
# ~/.config/newtricks/tricks.toml
[settings]
agents         = ["claude", "codex"]     # default agents for links (a source repo can set its own)
fetch_interval = "24h"
live           = ["skills.sh", "tessl", "clawhub", "github"]   # live-query adapters used by search

[source-repos]
personal = "~/code/my-skills"
team     = "~/code/acme-skills"

[catalogs]
"github.com/anthropics/skills"                             = {}                        # plain repo
"github.com/anthropics/claude-plugins-official"            = { kind = "marketplace" }
```

The first run writes this file with the settings' defaults and the **recommended catalogs**, so nothing search uses is hidden or built in: `catalog remove` removes one, and `catalog add --recommended` restores any that are missing. The user config holds no skills: New Tricks does not install skills on the workstation.

### Source repo manifest

```toml
# <source-repo>/tricks.toml
[source-repo]
agents = ["claude", "codex"]           # agents for links (default: the user setting)

[skills.pdf]
path     = "skills/pdf"
upstream = "github.com/anthropics/skills//skills/pdf"
track    = "main"                       # branch, "latest", or a version
update   = "review"                     # review | pinned | paused

[skills.invoice]
path     = "skills/invoice"
upstream = "clawhub.ai/acme/skills//invoice"   # catalog-hosted upstream (§5)

[skills.secret-sauce]
path     = "skills/secret-sauce"
upstream = "github.com/acme/skills//skills/secret-sauce"
license-override = { justification = "Separate redistribution agreement with Acme" }   # §11

[skills.deploy-aws]
path = "skills/deploy-aws"              # local original, no upstream

[lint]
ignore = ["NT305"]

[publish.targets.public]
repo    = "acme/acme-skills-public"     # owner/repo, host/owner/repo, any git URL, or a path
skills  = ["pdf", "deploy-aws"]
exclude = ["evals/**", "notes/**", "*.draft.md"]

[publish.targets.internal]
repo   = "git@git.acme.internal:skills/internal.git"
skills = ["*"]
```

### Lock

```toml
# source repo tricks.lock
[skills.pdf]
base      = "a1b2c3d…"                  # B: upstream commit last incorporated
base_tree = "9f3c…"

[skills.invoice]
base      = "clawhub:1.1.0"             # catalog-hosted: version or sha256 digest
base_tree = "71ae…"                     # snapshot kept in the store
```

### Local overrides

`tricks.work.toml` (gitignored, like `go.work`) holds machine-local overrides such as `use pdf@terse --local`, so personal experiments in a shared source repo never rewrite the committed `tricks.toml`.

## 7. Discovery and search

### Adapter model

- **Catalog adapters** return *pointers* plus catalog-specific signals (install counts, categories, "listed in").
- **The git adapter** resolves every pointer to real content — `SKILL.md` frontmatter and body plus the file tree — via the GitHub trees API and raw files (no full clones). Search always runs over actual content, normalized one way. The git adapter also indexes any repository added directly.
- The index lives in `state.db` (SQLite FTS5), is built locally, and works offline once cached. Private and organization catalogs work through the user's own credentials. A future hosted "super-index" can be added as another adapter.

### Adapter modes

- **Indexed**: fetched ahead of time (on `catalog add` and throttled refresh) into the local index.
- **Live-query**: called at search time when online, all enabled adapters concurrently; results are merged with indexed results and cached in the index with a TTL, so repeat and offline searches still find them.

### v1 adapters

| Adapter | Mode | Notes |
|---|---|---|
| Git repository | indexed | Resolves every pointer to content; also indexes any repository added directly |
| `marketplace.json` | indexed | One adapter covers Claude plugin marketplaces **and** APM marketplaces (APM uses the same format). Reads `plugins[].source`, `skills[]` and `<plugin>/skills` |
| skills.sh | live-query | Unauthenticated `GET https://skills.sh/api/search?q=&limit=` (the endpoint `npx skills find` uses). Returns `owner/repo`, name, install count — no path or description, which the git adapter fills in. Per-IP rate limit; the terms encourage caching. The bulk `/api/v1` listing requires a Vercel OIDC token, and sitemap crawling is not used |
| GitHub code search | live-query | `SKILL.md` search with the user's token |
| Tessl | live-query | Unauthenticated `GET https://api.tessl.io/experimental/search?q=&page[size]=`. Results are pointers into GitHub repositories (`sourceUrl` + `path`); only the pointed-to skill directories are indexed, since they often sit in large application repositories. Tessl's quality, aggregate score, security level and eval improvement are kept as listing signals; a MEDIUM/HIGH/CRITICAL security level becomes a risk flag |
| ClawHub | live-query | Unauthenticated `GET https://clawhub.ai/api/v1/search?nonSuspiciousOnly=true` (skills ClawHub flags as suspicious are not shown). Mirrors of GitHub skills (e.g. from skills.sh) become git pointers. Native skills are catalog-hosted (§5): indexed from the skill and version detail, installed from ClawHub's ZIP with every file verified against the version's published SHA-256 list. Installs, downloads, stars, moderation verdict, security status and VirusTotal verdict are listing signals |
| `.well-known/agent-skills/index.json` | indexed | agentskills.io discovery schema 0.2.0; covers any organization hosting its own index |
| Pointer lists: `apm.yml`, `skills-lock.json` | indexed | `tricks catalog add ./project/apm.yml` surfaces what a project or team already uses. Interop only: never touches installed files |

### Normalized record and facets

| Facet | Derived from |
|---|---|
| Listed in / owner / org | catalogs, repository |
| Trust: yours · your org · official · starred by you · unknown | owner, the user's GitHub identity, org membership and starred repositories |
| Agent compatibility | frontmatter fields, agent-specific keys |
| Risk surface: scripts, `allowed-tools`, network references | file tree and content scan |
| Popularity: installs (per catalog), stars | catalogs, GitHub |
| Freshness: last commit | git |
| Local state: linked · vendored · customized · has branches · upstream changes | `state.db` and the source repos (New Tricks's own records only) |
| License, category / tags | repository, catalogs |

### Deduplication and ranking

- Results are grouped by tree hash (identical copies) with near-identical forks noted: one result reads "in 4 catalogs · 6 forks · 2 customized versions".
- Ranking: full-text relevance (name, description, body) × trust × popularity × freshness.

## 8. Links: validating with real agents

Validation means watching real agents use a skill. `link` deploys source repo skills into agent skill directories — a project's or the user-level ones — and `unlink` removes them; `try` deploys a skill that is not in the source repo, and `untry` removes it. The two sets are kept apart so each list and each removal means one thing. Links are for testing, not installation: New Tricks never manages what is installed on the workstation (APM, `npx skills` and plugin marketplaces do), and it coexists with skills those tools put there.

```bash
tricks link                                        # in a source repo: every skill, user-level agent dirs, dev mode
tricks link pdf --to ~/code/sandbox-app            # one repo skill into one project
tricks link pdf@terse --to ~/code/sandbox-app      # this link deploys branch `terse`; others keep the default
tricks unlink                                      # in a source repo: all of its skills' links (--all: every source repo's)
tricks unlink pdf                                  # one skill; --to / --global narrow it
tricks list --links                                # this repo's links by place, with the branch each deploys (outside a repo: --all)

tricks try anthropics/skills//webapp-testing       # trial: a skill from elsewhere, into the current project
tricks untry webapp-testing                        # one trial, wherever it is; no skill: this project's trials
tricks list --trials                               # trials here and at user level (--all: everywhere)
```

| Command | Deploys | Default target |
|---|---|---|
| `link <skill>` (a source repo skill) | The skill's default: its active variant (§10), else the working tree (dev mode); follows `use`, `merge` and `update` automatically | User-level agent directories (`--global`) |
| `link <skill>@<branch>` | That branch, for this link only: the draft while `edit` has the branch checked out (dev mode), the working tree if it is the current branch, else a snapshot of the branch tip, refreshed when the branch moves. `link <skill>` un-pins it; merging the branch moves it to the default | as above |
| `try <upstream>` (**trial**) | An exact revision from the store (git or catalog-hosted, verified); never locked or updated | The current project |
| `try <folder>` (a local skill outside the repo) | Dev mode | The current project |

### Store

- Variants, snapshots of pinned branches, trials, merge snapshots and the base snapshots of catalog-hosted upstreams live in the read-only, content-addressed store. It is a cache: `unlink` prunes entries nothing references any more (`gc` is plumbing).
- Nothing, including an agent, can silently mutate a stored revision.
- Where an agent cannot follow links, the placement falls back to `copy` for that agent (see *Link capability* below).

### Deploy modes

Each link of a source repo skill deploys one branch, and links need not agree: a project can test `terse` while the user-level link stays on `main`. `list --links` shows each link's branch and how it gets there — `main (working tree, live)`, `terse (draft, live, pinned)`, `verbose @ 3f2a1c9 (snapshot, pinned)` — and `list` shows it per place (`linked: user level (main), app (terse)`).

| Mode | Behaviour | Use |
|---|---|---|
| `dev` | Link to a mutable source repo directory or worktree; edits are live | Authoring and branch experiments |
| `store` | Link to an immutable store revision | Variants, pinned branches not checked out, trials, merge snapshots |
| `copy` | Physical copy written to a temporary directory and renamed into place (atomic swap); drift is flagged | Agents or platforms that cannot follow links |

### Agents and placement

Skills are placed in the **primary** skill directory of each agent the user selects; when selected agents share a primary directory, one placement serves them. Other agents discovering skills through directories they also read is an accepted side effect; there is no assignment matrix and no strict-placement mode.

| Agent | Primary (user / project) | Also reads (user) | Links followed | Duplicates |
|---|---|---|---|---|
| Claude Code | `~/.claude/skills/` / `.claude/skills/` (cwd up to repo root) | — (does not read `.agents/skills`) | Yes (documented; verified locally) | Same target loads once; enterprise > personal > project |
| Codex | `~/.agents/skills/` / `.agents/skills/` (cwd up to repo root) | `~/.codex/skills/` (deprecated), `/etc/codex/skills` | Yes in user/repo/admin scope; not in system scope | Deduplicated by real `SKILL.md` path; same name at different paths shows both |
| Cursor | `~/.cursor/skills/` / `.cursor/skills/` (recursive, subtree-scoped) | `~/.agents/skills/`, `~/.claude/skills/`, `~/.codex/skills/` | Yes (IDE ≥ 2.5, CLI since May 2026); skips hidden dot-directories | Not documented |
| GitHub Copilot | `~/.copilot/skills/` / `.github/skills/` | `~/.claude/skills/`, `~/.agents/skills/` | CLI yes; VS Code partially (menu lists, `skill()` tool fails — microsoft/vscode#315979) | May list twice when directories link to each other |

Integrations declare their directories and capabilities; the table is a verified baseline (September 2026), not a hard-coded list. Each integration records the agent version it was tested against.

### Link capability and copy fallback

Each integration declares `follows_links` per platform. Where it is false, that agent's placement uses `copy` mode; the store remains the source of truth.

| | macOS | Linux | Windows |
|---|---|---|---|
| Claude Code | link | link | **copy** (junction-linked skills fail at session start — anthropics/claude-code#41177) |
| Codex | link | link | **copy** |
| Cursor | link | link (pending hidden-path test, §17) | **copy** |
| Copilot (VS Code) | **copy** until microsoft/vscode#315979 is fixed | **copy** | **copy** |
| Copilot (CLI) | link | link | **copy** |

Flipping a flag when an upstream fix ships is a one-line integration change.

### Git hygiene, collisions and records

- **Git hygiene**: placements inside a git repository are added to that repository's local exclude file (never `.gitignore`); nothing appears in `git status`.
  - Resolve the file with `git rev-parse --git-path info/exclude` — in linked worktrees and submodules `.git` is a file, not a directory.
  - All linked worktrees share the main repository's exclude file; submodules have their own (`.git/modules/<name>/info/exclude`).
  - Entries are anchored paths (`/.claude/skills/pdf`) inside a marked `# >>> new-tricks` … `# <<< new-tricks` block. Because the file is shared across worktrees, `unlink` removes an entry only when no other active placement (tracked in `state.db`) uses that path.
  - Targets outside git need no exclude handling.
- **Collisions**: if the target already has a skill with that name (for example one installed by another tool), `link` refuses. `--shadow` moves the existing folder to `backups/`, links in its place, and `unlink` restores it byte-for-byte.
- **Records**: every link (target, agents, skill, revision or worktree, timestamp) is stored in `state.db`; `tricks test` (§2) will attach results to them.
- **Stale links**: deleted targets or shadowed skills overwritten by another tool are reported by `list --links` / `--trials`, never silently re-applied.
- **Discoverability**: `list` shows where each repo skill is linked; every `link` and `try` result says how to see and remove it; `unlink` on a trial (and `untry` on a repo skill) names the right command.

## 9. Upstream tracking

Vendored skills record their upstream and the base (B) they last incorporated (§6). Upstream changes never reach a skill on their own: they are reported, reviewed and merged (§10).

### Policies

Per vendored skill, in the source repo's `tricks.toml`:

| `update =` | Behaviour |
|---|---|
| `review` (default) | Upstream changes are reported by `list` and `outdated`, and merged by `tricks update` |
| `pinned` | Stay on the recorded base; `tricks update` skips it unless the skill is named |
| `paused` | Do not check upstream |

### Freshness without a scheduler

Following Homebrew's model:

- `tricks outdated` (and `update`) fetch upstreams whose last fetch is older than `fetch_interval` (default 24 h; `--offline` skips) and report the incoming changes with a risk summary. Catalog-hosted upstreams are fetched from their catalog and verified (§5).
- `tricks list` reports upstream changes from the cached mirrors and index only, without touching the network.
- The VS Code extension runs the same throttled check on activation and on a timer while open, and updates its status bar item.
- An agent status-line integration (`tricks statusline`) reads cached state only and never touches the network.

### Risk scan

Every upstream change is scanned and summarized alongside the B → U diff: new or changed scripts, widened `allowed-tools`, new network or URL references, hidden or bidirectional Unicode. The same scanner powers lint's NT5xx rules and the publish risk diff.

Catalog security signals are added to the risk surface in search and preview: Tessl security level MEDIUM or above; ClawHub suspicious flag, malware block, non-clean moderation or security status, and a VirusTotal `malicious` verdict. VirusTotal `suspicious` is shown but not flagged, because it fires on any shell usage.

## 10. Source repo authoring

### Bringing skills in

```bash
tricks init [--agent-skill]                        # make the current git repo a source repo
tricks vendor anthropics/skills//pdf               # copy an upstream skill in, record upstream + base
tricks vendor clawhub.ai/acme/skills//invoice      # catalog-hosted upstreams too (§5)
tricks vendor anthropics/skills//pdf --from ~/old/pdf --base a1b2c3d   # a copy made earlier, at the revision it started from
tricks create my-skill                             # scaffold a local original
tricks create my-skill --from ~/somewhere/my-skill # or take an existing folder
tricks remove my-skill                             # unlink, delete, drop from manifest and lock (uncommitted)
tricks list                                        # skills and their state
```

Vendoring is copy-on-write: an upstream skill can be tried with `try` before deciding to customize it; `tricks edit` on an upstream skill offers to vendor it. Multiple source repos can be registered in the user config (typically one personal and one team repository).

`vendor` shows the upstream licence class (§11). For Block-class skills — e.g. terms forbidding derivative works — it requires confirmation ("terms may prohibit modification; you are responsible"). Search result cards show the licence class too.

### Upstream update

Precedent: `git subtree pull`, `copier update` / `cruft update`; the name follows `copier update` / `cruft update`, which likewise merge upstream changes into a customized copy.

1. New Tricks fetches the upstream into its fetch-only mirror (git), or fetches and verifies the latest revision from its catalog into the store (catalog-hosted upstreams, whose base snapshot the store keeps).
2. `tricks update pdf` computes B → U and three-way merges it into the working tree (C), producing R, and bumps `base` in the lock. `tricks outdated [--diff]` reports what would come in, with the risk summary; `tricks update --dry-run` shows the resulting R; neither changes anything.
3. The result is left **uncommitted** for review with `git diff` or VS Code's SCM view; the user commits with git.
4. Conflicts produce standard markers, open in VS Code's three-way merge editor, and continue with `tricks update --continue` or `--abort`.
5. Links keep the committed version (a store snapshot) until the merged result is committed; the next `tricks` command then returns them to the working tree.
6. File deletions versus local edits, renames and binary conflicts are surfaced explicitly. Upstream history rewrites or disappearance are reported; existing copies are kept.

Comparisons available in CLI and extension:

| View | Compare | Question |
|---|---|---|
| My customizations | B → C (`diff <skill> base..`) | What did I change? |
| Incoming | B → U (`outdated <skill> --diff`) | What did upstream change? |
| Candidate | C → R (`update <skill> --dry-run`) | What will change for me? |

`diff <skill> [<from>..<to>]` otherwise compares versions inside the source repo: branch or commit names, `head` and `working` (default `head..working`).

### Branch experiments

UX follows `pnpm patch`; mechanics are plain git branches and worktrees of the source repo. An experiment always has a branch; its worktree is checked out **inside the source repo** at `.tricks/work/<branch>/` (git-ignored), so it sits next to the skills in the editor and within the directory agents working in the repo may write to. Commands run inside a worktree act on its source repo.

```bash
tricks edit pdf                    # start (or continue) an experiment: branch draft/pdf
tricks edit pdf -b terse           # on branch `terse`
tricks link pdf@terse --to ~/code/app  # test the draft with agents in one project (live)
cd "$(tricks edit pdf)"            # stdout is the draft's path…
tricks edit pdf --shell            # …or open a shell there (`exit` returns)
tricks edit pdf --commit -m "…"    # commit the draft on its branch (with a Tricks-Agent trailer for agents)
tricks diff pdf head..terse        # compare
tricks use pdf@terse               # choose the default links deploy (committed in tricks.toml)
tricks use pdf@terse --local       # machine-only override in tricks.work.toml
tricks merge pdf@terse             # bring it back: only the skill's folder, as one commit
tricks merge pdf@terse --whole-branch   # ...or the whole branch
tricks merge pdf@terse --pr        # ...or as a pull request on the source repo's remote
tricks edit pdf --done             # stop editing without merging; links pinned to the branch deploy its tip
```

`merge` applies the skill's changes since the branch point as a three-way patch, so later changes on the current branch are kept; files the branch changed outside the skill are reported, not merged. Afterwards editing ends, a `use` of that branch is reset, and links pinned to the branch follow the default again (which now has the changes). `edit` never re-points links: only links pinned to the branch show the draft. Users may operate on the branches with git directly; New Tricks picks up the result.

### Lint

Ruff-style rules with stable codes, default severities, configuration under `[lint]`, per-skill overrides, and inline disables via frontmatter `metadata` (`tricks-lint-disable: NT203`, stripped at publish). `strict-spec = true` (or `tricks lint --strict`) makes any frontmatter key outside the Agent Skills spec an error, matching the reference validator exactly. Implemented natively in Rust; NT1xx is tested against the official `skills-ref` fixtures.

| Family | Examples | Default |
|---|---|---|
| **NT1xx** Spec conformance ([Agent Skills spec](https://agentskills.io/specification), cross-checked against the `skills-ref` test suite) | `name` format (lowercase letters of any script, digits, single hyphens; NFKC-normalized); name ≠ folder; `description` empty or > 1024; `compatibility` > 500; `metadata` not a string map; `skill.md` instead of `SKILL.md` (NT110, warning) | error |
| **NT2xx** Structure | broken relative links; missing referenced scripts; absolute or `~/` paths; references nested > 1 level; `SKILL.md` > 500 lines; body > ~5k tokens | error for broken links/files; warn for size/nesting |
| **NT3xx** Triggering quality | description lacks "use when…"; description < ~60 chars; duplicate `name` in the source repo; near-duplicate descriptions competing for triggers | warn; duplicate name is error |
| **NT4xx** Agent compatibility | agent-specific keys without that agent targeted; unknown keys (preserved); non-ASCII frontmatter (APM rejects it) | warn / info |
| **NT5xx** Safety | hidden/bidi Unicode; secret patterns; `curl … \| sh` or remote fetch in scripts; broad `allowed-tools` such as `Bash(*)` | error for Unicode and secrets; warn otherwise |

`--fix` applies only mechanical, unambiguous fixes (name casing, whitespace, line endings). It never rewrites prose. For upstream skills in search results, name/folder mismatch is shown as a warning only.

## 11. Publishing

The source repo is where skills are made; the publish target is distribution. Consumers never see drafts, experiments, vendoring bookkeeping or eval scaffolding. Precedent: Copybara; monorepo build → dist → publish.

### Output

```
acme-skills-public/
  skills/pdf/SKILL.md …              # Agent Skills layout → npx skills, APM, Copilot, Codex, Cursor
  .claude-plugin/marketplace.json    # generated → /plugin marketplace add acme/acme-skills-public
  apm.yml                            # generated, metadata only
  PROVENANCE / LICENSE / NOTICE      # generated attribution for vendored skills
  CHANGELOG.md                       # generated, grouped by skill
  .tricks-published              # paths owned by New Tricks
```

Verified installer behaviour for this layout:

- **`npx skills add owner/repo`** scans `skills/` (depth ≤ 3) and paths declared in `marketplace.json`; `skills/<name>/SKILL.md` is always discovered.
- **APM** installs `skills/<name>/SKILL.md` directly (`apm install owner/repo/skills/pdf`, or `apm install owner/repo --skill pdf`) without requiring `apm.yml`. It requires `name` = folder name and ASCII-only frontmatter (lint NT1xx/NT4xx).
- **Claude Code** needs no `plugin.json`: a marketplace entry with `"source": "./", "strict": false` loads skills from `skills/`, or exactly the listed `skills` paths.

**`marketplace.json`**: by default **one plugin containing every skill** in the target. Optional groups split skills into several plugins; skills not listed in any group go into the default plugin.

```toml
[publish.targets.public.plugins]      # optional
documents = ["pdf", "docx"]
devops    = ["deploy-aws"]
```

```json
{
  "name": "acme-skills-public",
  "owner": { "name": "Acme" },
  "plugins": [
    { "name": "acme-skills-public", "source": "./", "strict": false, "version": "1.3.0", "description": "…" }
  ]
}
```

With groups, each plugin entry lists its `skills` (`["./skills/pdf", "./skills/docx"]`). The marketplace name defaults to the target repository name, must be kebab-case, and is checked against Claude Code's reserved names (e.g. `agent-skills`, `anthropic-plugins`); a collision is a publish error.

**`apm.yml`**: metadata only — `name`, `version`, `description` (≤ ~80 chars, separate from any `SKILL.md` description), `license`. Not required by APM to install, but gives APM users a package name and the source repo version.

Transforms in v1 are limited to: dropping `exclude` globs, stripping New Tricks-only frontmatter keys, and carrying upstream licence and attribution. What was tested is what ships.

### Gates (in order)

1. Publish only from a clean, committed source repo, so provenance names an exact commit. `--dry-run` works on a dirty tree.
2. `tricks lint` has zero errors.
3. Licence gate for vendored skills, per the licence policy below. Target visibility is checked via the API, not trusted from config.
4. Leak check: secret patterns; warnings for files that look private to the source repo (`.env`, `notes/`, `*.draft.*`) not excluded.
5. Risk diff since last publish (e.g. "+1 script; `allowed-tools` widened on pdf") shown for confirmation.
6. *(Future)* eval gate.

### Licence policy

Real skill repositories make this necessary: `anthropics/skills` has no root licence, most skills are Apache-2.0, but `docx`, `pdf`, `pptx` and `xlsx` are "All rights reserved" with terms forbidding derivative works and distribution, and one skill has no licence at all; `openai/skills` includes proprietary Figma skills. GitHub's licence API only inspects the repository root and returns 404 for these repositories.

**Detection order** (implemented with the `spdx` crate, `detection-inline-cache` feature — the maintained successor to the archived askalono):

1. Licence file in the skill directory (`LICEN[CS]E*`, `COPYING*`, case-insensitive), text-matched with confidence ≥ 0.9; lower confidence, or text containing "All rights reserved" / restrictive terms, is treated as Proprietary/Unknown. `NOTICE*` files are carried.
2. Frontmatter `license:` — parsed as an SPDX expression (lax); otherwise keywords ("Proprietary", "All rights reserved" → Block; "terms in LICENSE" → defer to step 1).
3. Repository-root licence file from the vendored snapshot.
4. GitHub licence API as last resort (404 → no licence; `NOASSERTION` → unknown).
5. For catalog-hosted skills with no licence of their own: the catalog's publishing terms (ClawHub: "all skills published on ClawHub are licensed under MIT-0"). A skill's own restrictive licence still wins — re-uploads of proprietary skills stay blocked.

When these disagree the most restrictive result wins. The class, where it came from and the confidence are recorded in the source repo lock. For SPDX expressions, `OR` takes the most permissive branch and `AND` the most restrictive.

| Class | Examples | Public target | Private target |
|---|---|---|---|
| **Allow** (licence and NOTICE carried) | MIT, MIT-0, Apache-2.0, BSD-2/3-Clause, 0BSD, ISC, Zlib, Unlicense, CC0-1.0, CC-BY-4.0, BSL-1.0 | allow | allow |
| **Weak copyleft** | MPL-2.0, EPL-2.0, LGPL-2.1/3.0 | allow + warn | allow |
| **Strong copyleft** | GPL-2.0/3.0, AGPL-3.0, CC-BY-SA-4.0, EUPL-1.2 | requires `--accept-copyleft` | warn |
| **Block** | no licence, unknown / low confidence, Proprietary, restrictive `LicenseRef-*`, CC-BY-ND-*, BUSL-1.1, Elastic-2.0, SSPL-1.0 | block | warn (terms still forbid redistribution) |
| **Non-commercial** | CC-BY-NC-*, PolyForm-Noncommercial | block by default | warn |

Overrides are per skill in `tricks.toml` (`license-override = { justification = "…" }`), require a written justification (e.g. "separate agreement with vendor"), are shown in the publish pre-flight, and are never applied automatically. A blocked gate prints the exact snippet to add.

### Target repository behaviour

- `repo` names the remote, never a working copy. New Tricks keeps its own clone of each target in its data directory and resets it to the remote's default branch before every publish, so nothing left over from an earlier run (or a hand edit) can leak into a release. A relative path is resolved against the source repo root and treated like any other remote, so it has to accept pushes (a bare repository).
- New Tricks owns only the paths listed in `.tricks-published`; re-publish syncs exactly those (including removals of deselected skills) and never touches hand-added files.
- Each publish is committed with provenance trailers, e.g. `Tricks-Source: github.com/acme/my-skills@4e1f9a2`.
- A publish always lands on the remote: `--push` commits, tags and pushes the default branch; `--pr` pushes a `tricks/publish-*` branch and opens a pull request via `gh` for team review. One of the two is required (a commit left in New Tricks' private clone would be invisible); `--dry-run` previews without either.

### Versioning

- **One version per source repo.** All skills in a source repo release together. Separate version lines require separate source repos.
- `tricks publish <target> --bump major|minor|patch` tags the target `vX.Y.Z` (each target receives the same tag). Untagged publishes are allowed; consumers see Go-style pseudo-versions.
- The version is written into every published `SKILL.md` as `metadata.version`, and into the generated `apm.yml` and `marketplace.json`.
- `CHANGELOG.md` is generated from source repo commit messages since the last tag, grouped by skill.
- New Tricks suggests a bump — the largest across changed skills — and the user confirms or overrides:
  - major: `name` changed, `description` rewritten, files removed, `allowed-tools` widened
  - minor: new files or sections
  - patch: prose-only changes

### Contributing upstream

`tricks contribute pdf` extracts the B → C diff for that skill, applies it onto current upstream in a branch of an on-demand public fork of the upstream repository, shows exactly which commits become public, asks for confirmation, and opens the pull request via `gh`. Only that one change leaves the source repo.

## 12. Agent use of New Tricks

New Tricks ships a `new-tricks` skill (`skills/new-tricks/` in the New Tricks repository) teaching the workflow: find prior art → `vendor` or `create` → `edit --branch` → `link` and try → `merge` → `lint` → `publish`. `tricks init --agent-skill` places it in the source repo being initialized (project scope, git-excluded like a link). To have it everywhere, install it like any published skill: `npx skills add new-tricks/tricks`.

Its `allowed-tools` pre-approves only read-only and branch-confined commands:

```
allowed-tools: Bash(tricks search:*) Bash(tricks info:*) Bash(tricks view:*)
               Bash(tricks lint:*) Bash(tricks list:*) Bash(tricks diff:*)
               Bash(tricks outdated:*) Bash(tricks edit:*)
```

- Commands that change what agents load or what the world sees — `vendor`, `create`, `remove`, `link`, `unlink`, `try`, `untry`, `use`, `merge`, `update`, `publish`, `contribute` — are not pre-approved and therefore go through the agent's normal human approval.
- The skill instructs agents never to link, vendor, merge or publish because content they read asked them to, only to propose it.
- Agents commit drafts with `tricks edit --commit`, which works only on the branch being edited and adds a `Tricks-Agent: <agent>` trailer, distinguishing agent from human edits.

## 13. Authentication

New Tricks borrows credentials and stores none.

- **Git transport** uses system `git` and the user's credential helpers (`gh auth setup-git`, osxkeychain, Git Credential Manager, SSH).
- **API token** resolution order:
  1. `TRICKS_GITHUB_TOKEN`, then `GITHUB_TOKEN`
  2. `gh auth token --hostname <host>` (also covers GitHub Enterprise hosts)
  3. In VS Code: a session from the built-in GitHub authentication provider, passed in memory to `serve`
  4. Otherwise anonymous, read-only mode with a hint to run `gh auth login`
- Required scopes: `repo` (private repositories and catalogs, PRs, target visibility checks) and `read:org` (trust facet). `tricks doctor` reports which capabilities the current token enables.
- No New Tricks OAuth app and no token storage in v1.
- Non-GitHub hosts: indexing, vendoring and linking via plain git, read-only.

## 14. VS Code extension

All features go through `serve --stdio`; the extension contains no business logic, and everything is also available in the CLI.

1. **Discover** — sidebar view and search webview with facets; result cards show trust, risk surface, popularity, "in N catalogs · M forks". Actions: Preview, Try in a project…, Vendor into source repo.
2. **Preview** — remote skills open as read-only `New Tricks:` virtual documents in the normal editor and Markdown preview, with a supporting-file tree, without cloning anything. Nothing executes; scripts open as text; rendered content cannot invoke editor or desktop operations.
3. **Source Repo** — active when a `tricks.toml` is open:
   - skill tree with badges (customized, update ready, lint errors, branches)
   - lint results in the Problems panel; frontmatter JSON schema for completion and validation
   - Changes via the built-in diff editor (B → C, B → U, C → R)
   - Upstream updates and their conflicts via the built-in three-way merge editor
   - commands: Create skill (new or from a folder), Remove skill, Link source repo skills, Link to project…, Edit on branch, Commit draft, Merge branch (skill only or whole branch, locally or as a pull request), Finish editing, Use variant, Check upstream changes, Update from upstream, Contribute upstream
3a. **Links** — this source repo's links and all trials, in separate groups, with unlink / untry per item and for all.
   - Publish pre-flight panel: gate results, risk diff, bump suggestion, changelog preview
4. **Status bar** — one item, e.g. `3 upstream · ⚠ 1 lint · 4 links`; click opens the actions.

Not in v1: agent matrix, eval or metrics dashboards, custom editor, settings UI beyond VS Code's settings contribution.

## 15. CLI reference

| Group | Commands |
|---|---|
| Discover (anywhere) | `search <query> [--facet …]`, `info <skill>`, `view <skill> [file]` (prints the file; pipe it to render), `catalog add [--recommended] \| list \| remove \| refresh`, `try <skill> [--to <path> \| --global]`, `untry [skill] [--to \| --global \| --all]` |
| Skills in the source repo | `init [--agent-skill]`, `create <name> [--from <folder>]`, `vendor <upstream> [--from <copy> --base <rev>]`, `remove <skill>`, `list [--links \| --trials] [--all]` |
| Work on skills | `edit <skill> [-b <branch>] [--shell \| --commit -m … \| --done]`, `use <skill>@<branch> [--local \| --reset]`, `diff <skill> [<from>..<to>]`, `merge <skill>@<branch> [--whole-branch] [--pr]` |
| Upstream | `outdated [skill] [--diff]`, `update [skill] [--dry-run \| --continue \| --abort]`, `contribute <skill>` |
| Validate | `link [skill] [--to <path> \| --global] [--agents …] [--copy] [--shadow]`, `unlink [skill] [--to \| --global \| --all]` (source repo links only), `lint [--fix] [--strict]` |
| Ship | `publish <target> [--bump …] (--dry-run \| --push \| --pr)` |
| Maintain | `doctor`, `upgrade` |
| Plumbing (hidden) | `serve --stdio`, `statusline`, `gc` |

Global flags: `--json` on every read command, `--offline`, `--yes`, `--quiet`.

Short-name resolution: `pdf` alone resolves against the current source repo's `tricks.toml` keys, then falls back to the search index with an interactive picker in a TTY (error in `--json` / non-interactive mode).

## 16. Distribution

- Homebrew formula (`brew install new-tricks/tap/tricks`), signed release binaries for macOS, Linux and Windows, `tricks upgrade` for non-Homebrew installs.
- Platform-specific VSIX builds bundling the binary, published to the VS Code Marketplace and Open VSX. Extension updates carry binary updates.
- Name notes: formula `tricks` (was `newtricks` until 0.6), GitHub org `new-tricks` (repo `new-tricks/tricks`) and the VS Code publisher `newtricks` were free in September 2026. The earlier name *skillbench* was dropped because "SkillBench" is an existing brand shipping agent-skill marketplaces.

## 17. Resolved questions and remaining verification

### Resolved (September 2026 research)

| Question | Resolution | Where |
|---|---|---|
| Cursor discovery directories and links | `~/.cursor/skills` + reads `.agents`, `.claude`, `.codex` skills; links followed; skips hidden dirs | §8 |
| Link / junction support per agent | Per-agent `follows_links` capability with copy fallback; Windows and Copilot-in-VS-Code on copy | §8 |
| Codex duplicates and disables | Deduplicates by real path; disables not exposed (no assignment matrix) | §8 |
| skills.sh access | Unauthenticated search endpoint as a live-query adapter; no bulk listing, no crawling | §7 |
| Tessl and ClawHub access | Public, unauthenticated search APIs as live-query adapters; ClawHub-native skills are catalog-hosted with per-file SHA-256 verification | §5, §7 |
| APM marketplace format | Same `marketplace.json` format as Claude; one adapter | §7 |
| Licence detection | `spdx` crate, four-step detection, policy table, vendor-time warnings | §10, §11 |
| `.git/info/exclude` with worktrees / submodules | Resolve via `git rev-parse --git-path`; shared across worktrees; marked block with reference counting | §8 |
| Generated manifests | One layout serves `npx skills`, APM and Claude marketplaces; single-plugin default with optional groups; metadata-only `apm.yml` | §11 |
| `apm.yml` / `skills-lock.json` as search catalogs | Yes — pointer-list adapter in v1 | §7 |

### Verification tests scheduled in milestones

- **M2**: Cursor discovery with a link whose target is under a hidden directory (`~/.local/share` on Linux). If Cursor skips it, Cursor uses `copy` on Linux.
- **M2**: Windows copy-mode placement for all four agents, including atomic rename-swap on relink.
- **M2**: Watch microsoft/vscode#315979 (Copilot `skill()` tool with symlinked skills) and anthropics/claude-code#41177 (junctions); flip `follows_links` when fixed.
- **M2**: Codex file-watcher behaviour with linked skill directories (openai/codex#30795 reports spinning on very large link targets; store entries are single skill directories, so expected to be unaffected).
- **M4**: End-to-end install of a published target via `apm install`, `npx skills add` and `/plugin marketplace add` (acceptance scenario 8), run in CI.

## 18. Delivery

| # | Milestone | Contents | Proves |
|---|---|---|---|
| M1 | Identity + search | ID grammar and URL normalization; indexed and live-query adapter modes; git, `marketplace.json` (Claude + APM), skills.sh, Tessl, ClawHub and GitHub search (live), `.well-known`, and `apm.yml` / `skills-lock.json` pointer-list adapters; FTS5 index, facets, dedup, licence class; `search`, `info`, `view`, `catalog`; auth chain; `--json`. Works anonymously. | Federated search beats what exists |
| M2 | Links | Store in platform-native locations; placement for Claude Code, Codex, Copilot, Cursor with per-agent `follows_links` and copy fallback; `link/unlink` (source repo skills, trials) with exclude handling and `--shadow`; risk scan; §17 verification tests. Extension: Discover, Preview, Links, Status bar. | Safe trial and testing with real agents |
| M3 | Source repo authoring | `init/create/vendor/remove/list`; three-way upstream `update` (git and catalog-hosted upstreams); `edit/use/merge`, worktrees, `tricks.work.toml`; lint NT1–5xx; bundled agent skill. Extension: Source Repo view, Problems, diff and merge editors. | The customize-and-experiment loop |
| M4 | Publish | Targets, gates including licence policy, generated `marketplace.json` (single plugin + optional groups), metadata-only `apm.yml`, provenance, source repo versioning and changelog, `--push/--pr`, `tricks contribute`; three-installer end-to-end test in CI. Extension: Publish pre-flight. | Bridge to APM and the ecosystem |
| M5 | Distribution | Homebrew, signed binaries, `upgrade`, platform VSIX on both marketplaces, `doctor`. | People can get it |
| M6 | Tests and evals | `tricks test`: repo-defined cases run headless against real agents with a linked variant; pass rate, trigger rate, tokens and time per variant; optional publish gate. | Skills are validated, not just linted |

### Acceptance scenarios

1. `anthropics/skills//pdf` and its GitHub `/tree/…` URL resolve to the same canonical ID and commit.
2. A skill listed in three catalogs appears as one search result with forks grouped.
3. An upstream change outside a customized paragraph merges cleanly; an overlapping change produces a conflict in the merge editor and linked agents keep the committed version.
4. Upstream changes never reach a vendored skill without an explicit `update`; `pinned` and `paused` skills are skipped.
5. Linking into a project leaves `git status` clean; `--shadow` followed by `unlink` restores the original byte-for-byte.
6. An agent using the bundled skill can draft and commit on a branch without prompts, while `vendor`, `link`, `merge`, `update` and `publish` always require approval.
7. Publishing a vendored skill with an unknown upstream licence to a public target is blocked; re-publishing never deletes hand-added target files.
8. A published target installs successfully via `apm install`, `npx skills add` and `/plugin marketplace add`.

## 19. Decision log

| Decision | Rationale | Supersedes (v1 spec) |
|---|---|---|
| New Tricks for design time; APM for steady state | APM already covers project dependencies across nine agents; the unmet need is discovery, customization and authoring | Full lifecycle manager |
| Trials apart from links; experiments always on a branch, checked out in the repo (0.5) | One meaning per list and removal; drafts reachable from the editor and by agents confined to the repo | `unlink`/`list --links` for both (0.4); worktrees in the data directory |
| Commands named after what people guess (0.4, 0.6): `info`/`view`, `create`/`remove`/`list`, `try`, `outdated`/`update`, `merge` for branches, `upgrade` | npm, brew, cargo and git precedent; one meaning per verb; `merge` means branches, `update` means upstream, `upgrade` means New Tricks itself | `show`, `status`, `new`, `merge` for upstream (0.3); `sync`, `self-update` (0.5) |
| Each link deploys its own branch (0.6) | Comparing branches with real agents needs two links on different branches at once; `edit` re-pointing every link hid what agents were testing | One branch per skill for all links (0.5) |
| One scope: the source repo; no workstation skill management (0.3) | Installing, updating and pinning skills on a machine is what APM, `npx skills` and plugin marketplaces do; duplicating it diluted the focus and made commands mean different things inside and outside a repo | User-scope installs, update policies and rollback (0.1–0.2) |
| Build our own; interoperate with APM and `npx skills` | APM is Python without a library API and has no customization model | — |
| Rust core, CLI first, VS Code extension as sole GUI | Console-first requirement; developer audience lives in VS Code-family editors; built-in diff/merge editors | macOS desktop app |
| Library + CLI + stdio child process; no daemon, no scheduler | Works in CI and headless; nothing to install at OS level | Background helper, 24 h schedule |
| Identity = host + repo + `//` + path, Go-style `@ref` | Unique, host-agnostic, mirrors Go/Terraform conventions | Display-name-agnostic catalog IDs |
| `tricks.toml` / `.lock`, Cargo format, Go semantics | Avoids filename collisions (`skills-lock.json` already means two formats) | — |
| Source repo is the customized copy; New Tricks never pushes it | Removes remote-repo management; user owns git | One private GitHub repo per upstream |
| Vendoring with recorded base; merges left uncommitted | copier/cruft precedent; review through normal git tools | Automatic merge-and-activate |
| Upstream changes reach a vendored skill only through an explicit `update` | Skills are prompts for privileged agents; clean text merges say nothing about behaviour | Automatic updates |
| Content-addressed store (a cache) + links | Exact variants and trials, merge snapshots, no drift, shared across agents | — |
| Local federated index with adapters | No infrastructure, private catalogs for free, offline | — |
| Link for selected agents only; no matrix | Shared directories make per-agent exclusion unreliable; side effects accepted | Requested / discoverable / verified matrix |
| No adoption of existing installs | Entry via search or explicit `vendor <folder>` is simpler and sufficient | Discover-and-adopt flow |
| One version per source repo | Matches `apm.yml` and `marketplace.json` package versions; split source repos for separate lines | — |
| Name **New Tricks** (command `tricks`) | `skm` collides with a Homebrew formula and several agent-skill tools; `skillbench` is an existing brand (SkillBench) shipping agent-skill marketplaces; an abstract name leaves room beyond skills | "Skills Manager" |
| Per-agent `follows_links` with copy fallback; Windows copies | Junction and symlink bugs in Claude Code (Windows) and Copilot (VS Code) | Junctions on Windows |
| Platform-native, non-hidden data directory | Cursor skips hidden dot-directories | `~/.local/share` everywhere |
| skills.sh as live-query adapter | Bulk API requires Vercel OIDC; search endpoint is public and caching is encouraged | Bulk-indexed catalog |
| Licence policy with vendor-time warnings | Major skill repos mix Apache/MIT with proprietary, no-derivatives terms | Publish-only licence check |
| Single plugin per target by default, optional groups | Simplest install; grouping only where it adds value | — |
