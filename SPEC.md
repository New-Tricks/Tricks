# New Tricks — v1 specification

Status: design agreed; implemented as v0.1.0 — see [IMPLEMENTATION.md](IMPLEMENTATION.md) for status, verification, implementation decisions and known gaps.

Updated: 23 September 2026. Supersedes *Skills Manager — first-version specification* (17 September 2026).

## 1. Positioning

**New Tricks is the design-time workbench for agent skills on a developer's machine.** It finds skills across fragmented catalogs, lets you try them against real agents, customize them while still receiving upstream improvements, author your own, and publish a clean, installable repository.

It deliberately does **not** compete with steady-state installers. Projects commit an `apm.yml` and install with [APM](https://github.com/microsoft/apm); anyone can install published skills with `npx skills`, a Claude plugin marketplace, or by copying the directory. New Tricks is the tool you use *before* that point, and the one that makes a skills repository consumable by all of them at once.

| Stage | Tool |
|---|---|
| Discover, preview, trial | **New Tricks** (user scope) |
| Customize upstream skills, experiment, author, lint | **New Tricks** (source repo) |
| Publish a distribution repository | **tricks publish** |
| Install into projects, CI, other machines | APM, `npx skills`, plugin marketplaces |

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
- Federated search, preview, user-level installs, test links into arbitrary projects.
- Source repo authoring: vendor, import, new, lint, upstream merge, branch experiments.
- Publishing with gates, generated ecosystem manifests, versioning and changelog.
- Bundled agent skill so agents can use New Tricks safely.

### Explicitly out of v1

| Item | Status |
|---|---|
| Evaluations / test harness | Deferred. Deployment history is recorded from day one so it can attach later. |
| Per-version success metrics | Deferred (needs per-agent session-log parsing). Same history supports it. |
| Agent × skill assignment matrix | Dropped. Skills are linked for the selected agents; other agents reading shared directories is an accepted side effect. |
| Adopting existing local installations | Dropped. Skills enter via search or `import <folder>`. |
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
| **User scope** | The user's machine-level New Tricks state: user-scope installs, registered source repos, settings. Config (the *user config*) at `~/.config/newtricks/tricks.toml`. |
| **Source repo** | A git repository where skills are authored and customized, and published from. Contains `tricks.toml` and `tricks.lock`. The user owns its remote and pushes. |
| **Upstream** | The repository and path a vendored skill came from. |
| **Vendored skill** | A copy of an upstream skill inside a source repo, with its upstream and base commit recorded. |
| **Local original** | A skill authored in (or imported into) a source repo with no upstream. |
| **Catalog** | Anything New Tricks searches: a skill repository, or an index that points at skills in repositories (skills.sh, `marketplace.json`, APM marketplace, Tessl, ClawHub, GitHub search). Managed with `tricks catalog`. |
| **Store** | Immutable, content-addressed directory of exact skill revisions. |
| **Link** | A symlink (junction on Windows) from an agent skill directory to a store entry or a dev worktree. |
| **Target (link)** | A scope — `global` or a project path — plus a set of agents. |
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
│ New Tricks CLI      │ ──────────────────────▶ │  ─ store & deployment         │
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
  tricks.toml                    # user manifest (intent)
  tricks.lock                    # user lock (resolved)
<data-dir>/                          # see table below
  store/<tree-hash>/                 # immutable, read-only skill revisions
  repos/<host>/<owner>/<repo>/       # fetch-only upstream mirrors (never pushed)
  work/<source-repo>/<skill>/<branch>/ # git worktrees for branch experiments
  publish/<key>/                     # New Tricks' clones of publish targets
  backups/                           # originals displaced by --shadow
  state.db                           # index (FTS5), catalog, links, deployments, journal
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

Updates compare the published digest or latest version with the lock. A ClawHub skill that ClawHub itself serves from GitHub (no published version; the download is a GitHub handoff) is installed as the git skill it points to.

### Content identity

- **Version truth**: commit SHA. Tags and branches are labels.
- **Content hash**: git tree hash of the skill directory. Used for store addressing, integrity verification, and grouping identical copies across repositories and catalogs.
- **Upstream renames/moves** are detected with git rename tracking during updates and recorded as aliases.

## 6. Manifest and lock files

Conventions follow Cargo (TOML; intent in the manifest, resolved facts in the lock) with Go semantics (path identity, `@ref`, pseudo-versions for untagged commits). Filenames are tool-specific to avoid collisions: `tricks.toml`, `tricks.lock`, `tricks.work.toml`. Regular projects never contain these files — they commit `apm.yml`.

### User manifest

```toml
# ~/.config/newtricks/tricks.toml
[settings]
agents         = ["claude", "codex"]     # default agents for `add`
fetch_interval = "24h"
live           = ["skills.sh", "tessl", "clawhub", "github"]   # live-query adapters used by search

[source-repos]
personal = "~/code/my-skills"
team     = "~/code/acme-skills"

[catalogs]
"github.com/acme/skills"                                   = {}                        # plain repo
"github.com/anthropics/claude-plugins-official"            = { kind = "marketplace" }

[skills]
"github.com/anthropics/skills//skills/pdf"   = { version = "latest", agents = ["claude", "codex"] }
"github.com/acme/skills//skills/deploy-aws"  = { branch = "main", update = "auto" }
"github.com/randomdev/cool//skills/x"        = { branch = "main", update = "unsafe-auto" }
```

### Source repo manifest

```toml
# <source-repo>/tricks.toml
[source-repo]
agents = ["claude", "codex"]           # agents for dev deployments (default: the user setting)

[skills.pdf]
path     = "skills/pdf"
upstream = "github.com/anthropics/skills//skills/pdf"
track    = "main"
update   = "review"

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

### Locks

```toml
# user tricks.lock
[[skill]]
id     = "github.com/anthropics/skills//skills/pdf"
ref    = "v1.4.0"
commit = "…"
tree   = "…"

[[skill]]
id     = "github.com/acme/skills//skills/deploy-aws"
track  = "main"
commit = "3f2a1c9…"                     # as of last explicit `update`
tree   = "…"
```

```toml
# source repo tricks.lock
[skills.pdf]
base      = "a1b2c3d…"                  # B
base_tree = "9f3c…"
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
- **Live-query**: called at search time when online; results are merged with indexed results and cached in the index with a TTL, so repeat and offline searches still find them.

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
| Local state: installed · vendored · customized · has branches · update ready | `state.db` (New Tricks's own records only) |
| License, category / tags | repository, catalogs |

### Deduplication and ranking

- Results are grouped by tree hash (identical copies) with near-identical forks noted: one result reads "in 4 catalogs · 6 forks · 2 customized versions".
- Ranking: full-text relevance (name, description, body) × trust × popularity × freshness.

## 8. User installs and deployment

### Store and links

- Every deployed revision lives in the read-only, content-addressed store. Agent directories contain only links to store entries (or to dev worktrees).
- Updates and rollbacks are atomic link swaps; prior revisions stay in the store. `tricks gc` prunes revisions no manifest, lock, link or history entry references.
- Nothing, including an agent, can silently mutate a deployed revision.
- Where an agent cannot follow links, deployment falls back to `copy` for that agent (see *Link capability* below).

### Deploy modes

| Mode | Behaviour | Use |
|---|---|---|
| `store` (default) | Link to immutable store revision | Normal installs |
| `dev` | Link to a mutable source repo directory or worktree; edits are live | Authoring and branch experiments |
| `copy` | Physical copy written to a temporary directory and renamed into place (atomic swap); lock verifies tree hash and flags drift | Agents or platforms that cannot follow links |

In a source repo, `tricks install` links every skill in it in `dev` mode (like linking in pnpm workspaces).

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

### Test deployments to any target

```bash
tricks link ./skills/pdf --to ~/code/sandbox-app --agents claude,cursor
tricks link ./skills/pdf --global --agents claude,codex
tricks unlink ./skills/pdf --to ~/code/sandbox-app
tricks unlink --all
tricks status
```

- **Git hygiene**: placements inside a git repository are added to that repository's local exclude file (never `.gitignore`); nothing appears in `git status`.
  - Resolve the file with `git rev-parse --git-path info/exclude` — in linked worktrees and submodules `.git` is a file, not a directory.
  - All linked worktrees share the main repository's exclude file; submodules have their own (`.git/modules/<name>/info/exclude`).
  - Entries are anchored paths (`/.claude/skills/pdf`) inside a marked `# >>> new-tricks` … `# <<< new-tricks` block. Because the file is shared across worktrees, `unlink` removes an entry only when no other active placement (tracked in `state.db`) uses that path.
  - Targets outside git need no exclude handling.
- **Collisions**: if the target already has a skill with that name, `link` refuses. `--shadow` moves the existing folder to `backups/`, links in its place, and `unlink` restores it byte-for-byte.
- **Records**: every link (target, agents, skill, revision or worktree, timestamp) is stored in `state.db`. This is also the deployment history future metrics attach to.
- **Stale links**: deleted targets or shadowed skills overwritten by another tool are reported by `status`, never silently re-applied.

## 9. Updates

### Policies

Per skill, in `tricks.toml`:

| `update =` | Behaviour |
|---|---|
| `review` (default) | Fetch and prepare the candidate; never change a deployment until `tricks update` |
| `auto` | Deploy newer commits locally on fetch. Allowed only for repositories owned by the user or a GitHub org the user belongs to |
| `unsafe-auto` | `auto` for a third-party repository; the explicit spelling makes the risk visible in review |
| `pinned` | Stay on the locked revision |
| `paused` | Do not fetch |

### Freshness without a scheduler

Following Homebrew's model:

- `tricks install` fetches upstreams whose last fetch is older than `fetch_interval` (default 24 h; `--offline` skips). `auto` entries deploy to latest at user scope; `review` entries get a prepared candidate and are reported ("2 updates ready — run `tricks update`").
- `tricks update [skill]` applies reviewed updates and moves the lock. `tricks outdated` lists pending ones.
- The VS Code extension runs the same throttled check on activation and on a timer while open, and updates its status bar item.
- An agent status-line integration reads cached state only and never touches the network.

### Lock semantics

- `auto` updates move *local deployments* and are recorded in `state.db`; they never rewrite a committed lock. `tricks status` shows "N commits ahead of lock".
- `tricks install --frozen` deploys exactly the lock (CI, reproducibility).
- An explicit `tricks update` records a new baseline in the lock.

### Risk scan

Every prepared update is scanned and summarized alongside the B → U diff: new or changed scripts, widened `allowed-tools`, new network or URL references, hidden or bidirectional Unicode. The same scanner powers lint's NT5xx rules and the publish risk diff.

Catalog security signals are added to the risk surface in search and preview: Tessl security level MEDIUM or above; ClawHub suspicious flag, malware block, non-clean moderation or security status, and a VirusTotal `malicious` verdict. VirusTotal `suspicious` is shown but not flagged, because it fires on any shell usage.

## 10. Source repo authoring

### Bringing skills in

```bash
tricks init                                        # make the current git repo a source repo
tricks vendor anthropics/skills//pdf               # copy upstream skill in, record upstream + base
tricks import ~/somewhere/my-skill                 # local original
tricks import ~/somewhere/pdf --upstream anthropics/skills//pdf --base a1b2c3d
tricks new my-skill                                # scaffold
```

Vendoring is copy-on-write: third-party skills can be installed at user scope straight from upstream; a source repo copy is only created when the user chooses to customize (`tricks edit` on an upstream skill offers to vendor it). Multiple source repos can be registered in the user config (typically one personal and one team repository).

`vendor` shows the upstream licence class (§11). For Block-class skills — e.g. terms forbidding derivative works — it requires confirmation ("terms may prohibit modification; you are responsible"). Search result cards show the licence class too.

### Upstream merges

Precedent: `copier update` / `cruft`.

1. New Tricks fetches the upstream into its fetch-only mirror.
2. `tricks update pdf` computes B → U and three-way merges it into the working tree (C), producing R, and bumps `base` in the lock.
3. The result is left **uncommitted** for review with `git diff` or VS Code's SCM view; the user commits.
4. Conflicts produce standard markers, open in VS Code's three-way merge editor, and continue with `tricks update --continue` or `--abort`.
5. Deployed revisions are untouched until the merged result is committed and redeployed.
6. File deletions versus local edits, renames and binary conflicts are surfaced explicitly. Upstream history rewrites or disappearance are reported; existing copies are kept.

Comparisons available in CLI and extension:

| View | Compare | Question |
|---|---|---|
| My customizations | B → C | What did I change? |
| Incoming | B → U | What did upstream change? |
| Candidate | C → R | What will change for me? |

### Branch experiments

UX follows `pnpm patch`; mechanics are plain git branches and worktrees of the source repo.

```bash
tricks edit pdf                    # worktree for editing; deployment flips to dev mode; prints path
tricks edit pdf --branch terse     # same, on branch `terse`
tricks commit pdf -m "…"           # commit worktree; deployment returns to store revision
tricks use pdf@terse               # choose deployed variant (committed in tricks.toml)
tricks use pdf@terse --local       # machine-only override in tricks.work.toml
```

Users may operate on the branches with git directly; New Tricks picks up the result.

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

Overrides are per skill in `tricks.toml`, require a written justification (e.g. "separate agreement with vendor"), are shown in the publish pre-flight, and are never applied automatically.

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

`tricks pr pdf` extracts the B → C diff for that skill, applies it onto current upstream in a branch of an on-demand public fork of the upstream repository, shows exactly which commits become public, asks for confirmation, and opens the pull request via `gh`. Only that one change leaves the source repo.

## 12. Agent use of New Tricks

New Tricks ships a `new-tricks` skill (offered on first run and installed at user scope with `tricks agent-skill`; `tricks init --agent-skill` places it at project scope in the repository being initialized, git-excluded like a test link) teaching the workflow: search → preview → vendor → `edit --branch` → lint → `commit`.

Its `allowed-tools` pre-approves only read-only and branch-confined commands:

```
allowed-tools: Bash(tricks search:*) Bash(tricks show:*) Bash(tricks lint:*)
               Bash(tricks status:*) Bash(tricks outdated:*)
               Bash(tricks edit:*) Bash(tricks commit:*)
```

- Commands that change what agents load or what the world sees — `add`, `vendor`, `use`, `link`, `update`, `publish`, `pr` — are not pre-approved and therefore go through the agent's normal human approval.
- The skill instructs agents never to install, link or publish because content they read asked them to, only to propose it.
- Commits made via `tricks commit` carry a `Tricks-Agent: <agent>` trailer when an agent environment is detected (e.g. `CLAUDECODE`), distinguishing agent from human edits.

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
- Non-GitHub hosts: indexing and install via plain git, read-only.

## 14. VS Code extension

All features go through `serve --stdio`; the extension contains no business logic, and everything is also available in the CLI.

1. **Discover** — sidebar view and search webview with facets; result cards show trust, risk surface, popularity, "in N catalogs · M forks". Actions: Preview, Install for agents…, Vendor into source repo.
2. **Preview** — remote skills open as read-only `New Tricks:` virtual documents in the normal editor and Markdown preview, with a supporting-file tree, without cloning anything. Nothing executes; scripts open as text; rendered content cannot invoke editor or desktop operations.
3. **Source Repo** — active when a `tricks.toml` is open:
   - skill tree with badges (customized, update ready, lint errors, branches)
   - lint results in the Problems panel; frontmatter JSON schema for completion and validation
   - Changes via the built-in diff editor (B → C, B → U, C → R)
   - Update conflicts via the built-in three-way merge editor
   - commands: Edit on branch, Use variant, Link to project…
   - Publish pre-flight panel: gate results, risk diff, bump suggestion, changelog preview
4. **Status bar** — one item, e.g. `⟳ 3 updates · ⚠ 1 lint`, plus active dev links; click opens the relevant view.

Not in v1: agent matrix, eval or metrics dashboards, custom editor, settings UI beyond VS Code's settings contribution.

## 15. CLI reference

| Area | Commands |
|---|---|
| Discover | `search <query> [--facet …]`, `show <skill>`, `catalog add \| list \| remove \| refresh` |
| User scope | `add <skill>[@ref] [--agents …]`, `remove`, `install [--frozen]`, `update [skill]`, `outdated`, `status` (inside a source repo, `-g` selects user scope) |
| Source repo | `init [--agent-skill]`, `source-repos`, `vendor <skill>`, `import <folder> [--upstream … --base …]`, `new <name>`, `lint [--fix]` |
| Experiment | `edit <skill> [--branch b]`, `commit <skill> -m`, `use <skill>@<branch> [--local]` |
| Test deploy | `link <skill> (--to <path> \| --global) --agents …`, `unlink [--all]` |
| Upstream | `pr <skill>` |
| Publish | `publish <target> [--bump …] (--dry-run \| --push \| --pr)` |
| Plumbing | `serve --stdio`, `doctor`, `self-update`, `gc` |

Global flags: `--json` on every read command, `--offline`, `--yes`.

Short-name resolution: `pdf` alone resolves against the current source repo's `tricks.toml` keys, then user installs, then falls back to search with an interactive picker in a TTY (error in `--json` / non-interactive mode).

## 16. Distribution

- Homebrew formula (`brew install newtricks`), signed release binaries for macOS, Linux and Windows, `tricks self-update` for non-Homebrew installs.
- Platform-specific VSIX builds bundling the binary, published to the VS Code Marketplace and Open VSX. Extension updates carry binary updates.
- Name notes: formula `newtricks` (installs `tricks`), GitHub org `new-tricks` (repo `new-tricks/tricks`) and the VS Code publisher `newtricks` were free in September 2026. The earlier name *skillbench* was dropped because "SkillBench" is an existing brand shipping agent-skill marketplaces.

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
- **M2**: Windows copy-mode placement for all four agents, including atomic rename-swap on update.
- **M2**: Watch microsoft/vscode#315979 (Copilot `skill()` tool with symlinked skills) and anthropics/claude-code#41177 (junctions); flip `follows_links` when fixed.
- **M2**: Codex file-watcher behaviour with linked skill directories (openai/codex#30795 reports spinning on very large link targets; store entries are single skill directories, so expected to be unaffected).
- **M4**: End-to-end install of a published target via `apm install`, `npx skills add` and `/plugin marketplace add` (acceptance scenario 8), run in CI.

## 18. Delivery

| # | Milestone | Contents | Proves |
|---|---|---|---|
| M1 | Identity + search | ID grammar and URL normalization; indexed and live-query adapter modes; git, `marketplace.json` (Claude + APM), skills.sh, Tessl, ClawHub and GitHub search (live), `.well-known`, and `apm.yml` / `skills-lock.json` pointer-list adapters; FTS5 index, facets, dedup, licence class; `search`, `show`, `catalog`; auth chain; `--json`. Works anonymously. | Federated search beats what exists |
| M2 | User installs | Store in platform-native locations; placement for Claude Code, Codex, Copilot, Cursor with per-agent `follows_links` and copy fallback; user manifest and lock; `add/remove/install/update/outdated/status`; policies and risk scan; `link/unlink` with exclude handling and `--shadow`; §17 verification tests. Extension: Discover, Preview, Status bar. | Safe, reproducible install and trial |
| M3 | Source repo authoring | `init/vendor/import/new`; three-way upstream merge; `edit/commit/use`, worktrees, `tricks.work.toml`; lint NT1–5xx; bundled agent skill. Extension: Source Repo view, Problems, diff and merge editors. | The customize-and-experiment loop |
| M4 | Publish | Targets, gates including licence policy, generated `marketplace.json` (single plugin + optional groups), metadata-only `apm.yml`, provenance, source repo versioning and changelog, `--push/--pr`, `tricks pr`; three-installer end-to-end test in CI. Extension: Publish pre-flight. | Bridge to APM and the ecosystem |
| M5 | Distribution | Homebrew, signed binaries, `self-update`, platform VSIX on both marketplaces, `doctor`. | People can get it |

### Acceptance scenarios

1. `anthropics/skills//pdf` and its GitHub `/tree/…` URL resolve to the same canonical ID and commit.
2. A skill listed in three catalogs appears as one search result with forks grouped.
3. An upstream change outside a customized paragraph merges cleanly; an overlapping change produces a conflict in the merge editor and the deployed revision stays intact.
4. A `review` skill never changes a deployment without explicit `update`; `auto` never rewrites a committed lock.
5. Linking into a project leaves `git status` clean; `--shadow` followed by `unlink` restores the original byte-for-byte.
6. An agent using the bundled skill can edit and commit on a branch without prompts, while `add`, `link` and `publish` always require approval.
7. Publishing a vendored skill with an unknown upstream licence to a public target is blocked; re-publishing never deletes hand-added target files.
8. A published target installs successfully via `apm install`, `npx skills add` and `/plugin marketplace add`.

## 19. Decision log

| Decision | Rationale | Supersedes (v1 spec) |
|---|---|---|
| New Tricks for design time; APM for steady state | APM already covers project dependencies across nine agents; the unmet need is discovery, customization and authoring | Full lifecycle manager |
| Build our own; interoperate with APM and `npx skills` | APM is Python without a library API and has no customization model | — |
| Rust core, CLI first, VS Code extension as sole GUI | Console-first requirement; developer audience lives in VS Code-family editors; built-in diff/merge editors | macOS desktop app |
| Library + CLI + stdio child process; no daemon, no scheduler | Works in CI and headless; nothing to install at OS level | Background helper, 24 h schedule |
| Identity = host + repo + `//` + path, Go-style `@ref` | Unique, host-agnostic, mirrors Go/Terraform conventions | Display-name-agnostic catalog IDs |
| `tricks.toml` / `.lock`, Cargo format, Go semantics | Avoids filename collisions (`skills-lock.json` already means two formats) | — |
| Source repo is the customized copy; New Tricks never pushes it | Removes remote-repo management; user owns git | One private GitHub repo per upstream |
| Vendoring with recorded base; merges left uncommitted | copier/cruft precedent; review through normal git tools | Automatic merge-and-activate |
| Default update policy `review`; `auto` only for own/org repositories; `unsafe-auto` spelled out | Skills are prompts for privileged agents; clean text merges say nothing about behaviour | Automatic by default |
| Content-addressed store + links | Atomic updates and rollback, no drift, shared across agents | — |
| Local federated index with adapters | No infrastructure, private catalogs for free, offline | — |
| Link for selected agents only; no matrix | Shared directories make per-agent exclusion unreliable; side effects accepted | Requested / discoverable / verified matrix |
| No adoption of existing installs | Entry via search or explicit import is simpler and sufficient | Discover-and-adopt flow |
| One version per source repo | Matches `apm.yml` and `marketplace.json` package versions; split source repos for separate lines | — |
| Name **New Tricks** (command `tricks`) | `skm` collides with a Homebrew formula and several agent-skill tools; `skillbench` is an existing brand (SkillBench) shipping agent-skill marketplaces; an abstract name leaves room beyond skills | "Skills Manager" |
| Per-agent `follows_links` with copy fallback; Windows copies | Junction and symlink bugs in Claude Code (Windows) and Copilot (VS Code) | Junctions on Windows |
| Platform-native, non-hidden data directory | Cursor skips hidden dot-directories | `~/.local/share` everywhere |
| skills.sh as live-query adapter | Bulk API requires Vercel OIDC; search endpoint is public and caching is encouraged | Bulk-indexed catalog |
| Licence policy with vendor-time warnings | Major skill repos mix Apache/MIT with proprietary, no-derivatives terms | Publish-only licence check |
| Single plugin per target by default, optional groups | Simplest install; grouping only where it adds value | — |
