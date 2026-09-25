---
title: Configuration
description: Every file, key and environment variable New Tricks reads, with defaults and complete annotated examples.
---

New Tricks reads four files: the source repo manifest `tricks.toml`, its generated `tricks.lock`, the machine-local `tricks.work.toml`, and your user config. All are TOML. Only the keys on this page are read; New Tricks ignores anything else.

| File | Where | Committed | Written by |
|---|---|---|---|
| [`tricks.toml`](#source-repo-manifest-trickstoml) | Source repo root | Yes | You, `init`, `create`, `vendor`, `remove`, `use` |
| [`tricks.lock`](#lock-file-trickslock) | Source repo root | Yes | New Tricks only |
| [`tricks.work.toml`](#local-overrides-tricksworktoml) | Source repo root | No (git-ignored) | `use --local` |
| [User config](#user-config) | `~/.config/newtricks/tricks.toml` | No | You, first run, `init`, `catalog` |

When New Tricks edits `tricks.toml` or the user config, it keeps your comments and formatting.

## Source repo manifest: `tricks.toml`

The manifest holds intent: which skills the repo has, where they came from, and how to lint and publish them. See [Source repo](/tricks/concepts/source-repo/).

### `[source-repo]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `name` | string | The directory name | The repo's name: its key in the user config's `[source-repos]`, and the fallback for the marketplace owner and description when publishing. Set by `init --name`. |
| `agents` | array of strings | The user setting `settings.agents` | Agents to link this repo's skills for: `claude`, `codex`, `cursor`, `copilot`, or `"all"`. `link --agents` overrides it for one command. See [Agents](/tricks/concepts/agents/). |

### `[skills.<name>]`

One table per skill. The table key is the skill's name in the source repo, which is also the directory name its links get in agent directories.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `path` | string | Required | The skill's directory, relative to the repo root. `create` and `vendor` use `skills/<name>`. |
| `upstream` | string | None (an original) | Canonical ID of the upstream skill, `host/owner/repo//path`, or a catalog-hosted ID such as `clawhub.ai/<owner>/skills//<slug>`. Set by `vendor`. See [Upstream tracking](/tricks/concepts/upstream/). |
| `track` | string | `"latest"` | What `outdated` and `update` compare against: `"latest"` (the highest semver tag, else the default branch), a version such as `"v1.2.0"` (that tag), or a branch name. `vendor` writes the branch when you vendor at `@<branch>`, else `"latest"`. |
| `update` | string | `"review"` | Update policy for a vendored skill. `review`: upstream changes are reported by `list` and `outdated` and merged by `update`. `pinned`: stays on its base; `update` with no skill skips it, and `outdated` notes the new version. `paused`: not checked; `outdated` and `update` skip it unless you name it. |
| `use` | string | None (the current checkout) | Branch whose version this skill's links deploy by default. Set by [`use`](/tricks/reference/commands/use/) and cleared by `use --reset`. A value in `tricks.work.toml` takes precedence. See [Branch experiments](/tricks/concepts/branch-experiments/). |
| `license-override` | inline table | None | `{ justification = "…" }`: allows publishing a vendored skill whose licence would otherwise block it; the licence gate then reports it as allowed by override, as a warning. Record why you have permission in the justification. A blocked licence gate prints the exact snippet to add. See [Publishing](/tricks/concepts/publishing/). |

### `[lint]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `ignore` | array of strings | `[]` | Rule codes to skip in every skill, such as `"NT305"`. Codes must match exactly. |
| `strict-spec` | boolean | `false` | Treat any frontmatter key outside the Agent Skills spec as an error, as the reference validator does. Same as `lint --strict`. |
| `per-skill.<name>.ignore` | array of strings | `[]` | Rule codes to skip for one skill, in a `[lint.per-skill.<name>]` table. |

A skill can also disable rules inline in its frontmatter, with `tricks-lint-disable` under `metadata` (codes separated by commas or spaces); the key is stripped when publishing. See [Lint](/tricks/concepts/lint/) and the [rule table](/tricks/reference/lint-rules/).

### `[publish.targets.<target>]`

One table per publish target; `<target>` is the name you pass to `tricks publish`.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `repo` | string | Required | The distribution repository: `owner/repo`, `host/owner/repo`, a git URL, or a path (relative paths resolve against the repo root) to a repository that accepts pushes, such as a bare repository. |
| `skills` | array of strings | `["*"]` | Skills to publish, by name; `"*"` means all of them. |
| `exclude` | array of strings | `[]` | Globs of files to leave out, relative to each skill directory, such as `"evals/**"`. |
| `plugins` | table of arrays | `{}` | Optional plugin groups for the generated `marketplace.json`, in a `[publish.targets.<target>.plugins]` table: `group = ["skill", …]`. Skills not in a group go into the default plugin. |
| `marketplace` | string | The target repository's name | Marketplace name in `marketplace.json`. Must be kebab-case and not a name Claude Code reserves. |
| `owner` | string | Your `git config user.name`, else the source repo name | Marketplace owner name. |
| `description` | string | `"Agent skills published from <name>"` | Plugin description in `marketplace.json`. |

### Complete example

```toml
# ~/code/my-skills/tricks.toml

[source-repo]
name   = "my-skills"                  # default: the directory name
agents = ["claude", "codex"]          # default: settings.agents from the user config

[skills.changelog-writer]             # an original: no upstream
path = "skills/changelog-writer"
use  = "terse"                        # links deploy branch `terse` by default

[skills.skill-creator]                # a vendored skill
path     = "skills/skill-creator"
upstream = "github.com/anthropics/skills//skills/skill-creator"
track    = "latest"                   # highest semver tag, else the default branch
update   = "review"                   # review | pinned | paused

[skills.invoice]                      # catalog-hosted upstream
path     = "skills/invoice"
upstream = "clawhub.ai/acme/skills//invoice"
license-override = { justification = "Separate redistribution agreement with Acme" }

[lint]
ignore      = ["NT305"]               # skip everywhere
strict-spec = false                   # true: unknown frontmatter keys are errors

[lint.per-skill.skill-creator]
ignore = ["NT206"]                    # skip for this skill only

[publish.targets.public]
repo        = "acme/my-skills-public" # owner/repo, host/owner/repo, git URL or path
skills      = ["*"]                   # default: all skills
exclude     = ["evals/**", "notes/**", "*.draft.md"]
marketplace = "acme-skills"           # default: the target repository's name
owner       = "Acme"                  # default: git config user.name
description = "Acme's agent skills"

[publish.targets.public.plugins]      # optional groups in marketplace.json
documents = ["invoice"]
authoring = ["skill-creator", "changelog-writer"]
```

## Lock file: `tricks.lock`

Generated; don't edit it. It records what New Tricks resolved for each vendored skill, so `update` knows the common ancestor for its three-way merge and `publish` knows each skill's licence. Originals have no entry.

| Key | Type | Meaning |
|---|---|---|
| `version` | integer | Lock format version, currently `1`. |
| `skills.<name>.base` | string | The upstream revision last incorporated: a commit SHA, `clawhub:<version>` or `sha256:<digest>` for catalog-hosted upstreams. |
| `skills.<name>.base_tree` | string | Git tree hash of the upstream skill directory at `base`. `list` compares it with your copy to decide whether the skill is `customized`. |
| `skills.<name>.upstream_path` | string | The upstream's new path, if upstream renamed or moved the skill since you vendored it. |
| `skills.<name>.license` | table | Detected licence: `spdx` (the SPDX expression, if any), `class` (`allow`, `weak-copyleft`, `strong-copyleft`, `non-commercial` or `block`), `source` (where it was found: `skill-file`, `frontmatter`, `repo-root`, `github-api` or `catalog-terms`) and `confidence` (0 to 1). |

```toml
# Generated by New Tricks. Do not edit.
version = 1

[skills.skill-creator]
base = "33375500bcea98d610eb30ce10ac4e59b89c390d"
base_tree = "3cf9a8db32597ba3e24b584a3d696f4e11c7d7b6"

[skills.skill-creator.license]
spdx = "Apache-2.0"
class = "allow"
source = "skill-file"
confidence = 1.0
```

## Local overrides: `tricks.work.toml`

Machine-local and git-ignored (`init` adds it to `.gitignore`), so personal experiments in a shared source repo never change the committed manifest.

| Key | Type | Meaning |
|---|---|---|
| `use.<skill>` | string | Branch whose version this skill's links deploy on this machine. Written by `use <skill>@<branch> --local`; overrides `use` in `tricks.toml`. |

```toml
[use]
changelog-writer = "terse"
```

## User config

Machine-level settings, the catalogs `search` draws on, and your registered source repos. It holds no skills: New Tricks does not install skills on your machine.

| Platform | Path |
|---|---|
| macOS, Linux | `$XDG_CONFIG_HOME/newtricks/tricks.toml`, default `~/.config/newtricks/tricks.toml` |
| Windows | `%APPDATA%\newtricks\tricks.toml` |

The first run writes this file with the default settings and the recommended catalogs, so nothing search uses is hidden. An existing file is never rewritten.

### `[settings]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `agents` | array of strings | `["claude"]` | Agents to link and try skills for when neither the command (`--agents`) nor the source repo (`[source-repo] agents`) says: `claude`, `codex`, `cursor`, `copilot`, or `"all"`. |
| `fetch_interval` | string | `"24h"` | How long fetched catalogs and upstreams count as fresh. A number and a unit: `s`, `m`, `h` or `d`, such as `"30m"`. |
| `live` | array of strings | `["skills.sh", "tessl", "clawhub", "github"]` | Live-query catalogs asked on every search. `[]` turns them all off; `search --no-live` skips them for one search. |

### `[source-repos]`

Registered source repos, `name = "path"`. `init` adds the repo it initializes; delete a line to unregister one. Outside a source repo, `list` shows these, and `list --links --all`, `unlink --all` and `doctor` use them. See [Source repo](/tricks/concepts/source-repo/#registered-source-repos).

### `[catalogs]`

Where `search` looks, one entry per catalog: `"<key>" = {}` or `"<key>" = { kind = "…" }`. Manage them with [`tricks catalog`](/tricks/reference/commands/catalog/); `catalog add --recommended` restores any missing recommended ones.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `kind` | string | Detected from the key | `repo`: a skill repository (`github.com/owner/repo`). `marketplace`: a Claude or APM `marketplace.json` repository. `wellknown`: a site serving `/.well-known/agent-skills/index.json` (keys starting with `https://` on non-GitHub hosts). `pointers`: an `apm.yml` or `skills-lock.json` file (keys ending in those names). |

See [Discovery](/tricks/concepts/discovery/).

### Complete example

```toml
# ~/.config/newtricks/tricks.toml

[settings]
agents = ["claude", "codex"]          # default: ["claude"]
fetch_interval = "24h"                # s | m | h | d
live = ["skills.sh", "tessl", "clawhub", "github"]   # [] for none

[source-repos]
my-skills = "~/code/my-skills"
team      = "~/code/acme-skills"

[catalogs]
"github.com/anthropics/skills" = {}                     # recommended
"github.com/openai/skills" = {}                         # recommended
"github.com/vercel-labs/agent-skills" = {}              # recommended
"github.com/github/awesome-copilot" = {}                # recommended
"github.com/obra/superpowers" = {}                      # recommended
"github.com/anthropics/claude-plugins-official" = { kind = "marketplace" }
"https://skills.example.com" = { kind = "wellknown" }
"~/code/my-app/apm.yml" = { kind = "pointers" }
```

## Environment variables

| Variable | Meaning |
|---|---|
| `TRICKS_HOME` | The home directory New Tricks uses: user-level agent directories (`~/.claude/skills` and so on), `~` in config paths, and the default config and data locations. Default: your home directory. |
| `TRICKS_CONFIG_DIR` | Directory holding the user config `tricks.toml`. |
| `TRICKS_DATA_DIR` | Data directory (see [below](#data-directory)). |
| `XDG_CONFIG_HOME` | On macOS and Linux, the user config lives in `$XDG_CONFIG_HOME/newtricks` when set. |
| `XDG_DATA_HOME` | On Linux, the data directory is `$XDG_DATA_HOME/newtricks` when set. |
| `TRICKS_GITHUB_TOKEN` | GitHub API token; checked first. |
| `GITHUB_TOKEN` | GitHub API token; checked after `TRICKS_GITHUB_TOKEN`, before `gh auth token`. |
| `TRICKS_NO_GH` | When set, don't ask `gh auth token` for a token. |
| `TRICKS_LINK_MODE` | Override how skills are placed: `copy` (every agent copies), `link` (every agent links), or per agent, `agent=mode,…` such as `copilot=link,claude=copy`. Unlisted agents keep their default. See [Agents](/tricks/concepts/agents/). |
| `TRICKS_RELEASE_REPO` | Repository `tricks upgrade` downloads releases from. Default: `new-tricks/tricks`. |

For hermetic tests and CI, New Tricks also reads `TRICKS_HOST_MAP` (map a host to a local directory of repositories, `github.com=/path/to/fixtures`, where `<dir>/<owner>/<repo>` are git repositories), `TRICKS_NO_API` (index GitHub repositories with git instead of the GitHub API), `TRICKS_SKILLS_SH_URL`, `TRICKS_TESSL_URL` and `TRICKS_CLAWHUB_URL` (catalog endpoints), and `TRICKS_IDENTITY` and `TRICKS_STARRED` (a fixed GitHub identity and starred list for the trust facet).

New Tricks sets two variables itself: `TRICKS_EDITING=<skill>@<branch>` in the shell that `edit --shell` opens, and `TRICKS_VSCODE_TOKEN` when the VS Code extension passes its GitHub session to the binary.

## Data directory

Caches and state live in a platform-native, non-hidden location where the platform allows, because Cursor skips skills behind hidden directories:

| Platform | Data directory |
|---|---|
| macOS | `~/Library/Application Support/newtricks` |
| Linux | `$XDG_DATA_HOME/newtricks`, default `~/.local/share/newtricks` |
| Windows | `%LOCALAPPDATA%\newtricks` |

| Inside | What |
|---|---|
| `store/` | Read-only, content-addressed skill revisions: trials, pinned-branch snapshots, catalog-hosted bases. A cache; `unlink` prunes it. |
| `repos/<host>/<owner>/<repo>/` | Fetch-only mirrors of upstream repositories. Never pushed. |
| `publish/` | New Tricks' own clones of publish targets, reset to the remote before every publish. |
| `backups/` | Skills moved aside by `link --shadow`, restored on unlink. |
| `state.db` | SQLite: search index, links and trials, fetch times, interrupted operations. |

[`tricks doctor`](/tricks/reference/commands/doctor/) shows the config and data directories in use.
