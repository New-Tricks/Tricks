---
title: Discovery
description: Search every skill catalog at once, identify skills precisely, and inspect a skill's licence, risk and content before you bring it in.
---

Before you write a skill, look for prior art. Skills are scattered across git repositories, plugin marketplaces, skills.sh, Tessl, ClawHub and organization sites. New Tricks searches all of them with one query, reads the real `SKILL.md` behind every listing, and shows identical copies as one result. Then `info` and `view` let you inspect a skill without cloning or running anything.

## Search

```bash
tricks search pdf forms --license allow --no-scripts
```

```text
pdftk-server  github/awesome-copilot//skills/pdftk-server
    Skill for using the command-line tool pdftk (PDFtk Server) for working with PDF files. Use when asked to merge PDFs, split PDFs, rotate pages, encrypt or decryp
    [official · licence:allow · 10.1k installs · ★39.4k]  in 1 catalog(s)
view-pdf  anthropics/knowledge-work-plugins//pdf-viewer/skills/view-pdf
    Interactive PDF viewer. Use when the user wants to open, show, or view a PDF and collaborate on it visually — annotate, highlight, stamp, fill form fields, plac
    [official · licence:allow · 6.1k installs · ★25.6k]  in 2 catalog(s)
PDF Form Filler  claude-office-skills/skills//pdf-form-filler
    Fill out PDF forms programmatically and extract form data
    [unknown · licence:allow · 4.1k installs · ★485]  in 2 catalog(s)
Extract PDF Text  clawhub.ai/ivangdavila/skills//extract-pdf-text
    Extract text from PDF files using PyMuPDF. Parse tables, forms, and complex layouts. Supports OCR for scanned documents.
    [unknown · licence:allow · 78 installs]  in 1 catalog(s) · network references
…
```

Each result shows the name and [skill identifier](#skill-identifiers), the start of the description, and a tag line: trust (`yours`, `org`, `official`, `starred` or `unknown`), licence class, installs, repository stars, and Tessl's quality score where Tessl lists it. After the tags come the number of catalogs listing the skill, identical copies, variants, and the risk surface: `scripts`, `allowed-tools` (or `broad allowed-tools`), `network references`, and any catalog security flags.

Every term must match, and the last term matches as a prefix. Results rank by text relevance (name first, then description, then body), weighted by trust, popularity and freshness. An empty query lists everything in the index.

### Filters

| Flag | Keeps |
|---|---|
| `--license <class>` | `allow`, `weak-copyleft`, `strong-copyleft`, `non-commercial`, `block` or `unknown` |
| `--no-scripts` | skills that ship no scripts |
| `--trust <level>` | `yours`, `org`, `official`, `starred` or `unknown` |
| `--agent <agent>` | skills that declare support for this agent (`claude`, `codex`, `copilot`, `cursor`) |
| `--owner <owner>` | skills from this owner's repositories |
| `--catalog <catalog>` | skills listed in this catalog, or from this repository |
| `--category <category>` | skills in this catalog category |
| `--limit <n>` | at most `n` results (default 20) |
| `--no-live` | skip the live catalogs for this search |
| `--refresh` | re-index every catalog first |

Trust comes from the owner: your GitHub login is `yours`, your organizations are `org`, repositories you starred are `starred`, and a short list of vendors (Anthropic, OpenAI, GitHub, Microsoft, Vercel, Google, Cursor) is `official`.

:::note
The licence class in search results is a quick estimate from frontmatter and repository metadata. A skill that keeps its licence in a `LICENSE.txt` file can show `licence:unknown` in search. `tricks info` runs the full detection, and that is what `vendor` and `publish` use.
:::

### Identical copies and variants

Popular skills are copied into many repositories and listed in several catalogs. New Tricks groups results by the git tree hash of the skill directory, so byte-identical copies collapse into one result that says `3 identical cop(ies)` and merges their catalog listings. Skills with the same name but different content are counted as variants (`16 variant(s)`): forks and customized versions you may want to compare.

### Where search looks

Two kinds of catalog feed the index:

- **Indexed catalogs** are listed in your user config and fetched ahead of time: skill repositories, `marketplace.json` files (Claude plugin marketplaces and APM marketplaces share the format), `.well-known/agent-skills` sites, and `apm.yml` or `skills-lock.json` pointer lists. They are re-indexed when older than `fetch_interval` (24 hours by default), so the first search on a new machine is the slow one.
- **Live catalogs** are asked on every search: skills.sh, Tessl, ClawHub and GitHub code search. They run at the same time, and their answers are cached for six hours per query. GitHub code search needs a token (`GITHUB_TOKEN` or `gh auth login`); without one it is skipped.

Whatever a catalog returns, New Tricks resolves the listing to the actual skill directory and indexes its real content. ClawHub skills that ClawHub flags as suspicious are not shown.

### Offline

The index lives on your machine. `--offline` searches it without touching the network, which takes milliseconds:

```bash
tricks search pdf --offline --limit 3
```

Skills found by earlier live searches stay in the index, so offline searches find them too.

## Skill identifiers

A skill is identified by host, repository and path, never by its name alone, because names repeat across repositories:

```text
[host/]owner/repo//path-or-name[@ref]
```

| You write | Means |
|---|---|
| `anthropics/skills//skill-creator` | `github.com/anthropics/skills`, the skill named `skill-creator` |
| `anthropics/skills//skills/skill-creator` | the same skill, by its exact path |
| `git.example.com/acme/skills//docx@v1.0.0` | another host, at tag `v1.0.0` |
| `https://github.com/anthropics/skills/tree/main/skills/pdf` | a pasted GitHub URL (`/blob/…/SKILL.md` works too) |
| `clawhub.ai/awspace/skills//pdf` | a skill hosted on ClawHub (its page URL works too) |
| `example.com/.well-known/agent-skills//invoice` | a skill from an organization's `.well-known` index |

- `//` is required: it marks where the repository ends and the path inside it begins. Without it, the reference names a repository.
- After `//`, New Tricks tries an exact path, then a frontmatter `name`, then a folder name. If more than one skill matches, it lists the candidates.
- A first segment containing a dot is a host; otherwise the host is `github.com`.
- `@ref` is a tag, branch (slashes allowed), commit (at least 7 characters) or `latest`. No ref means `@latest`: the highest semver tag, or the default branch when there are no tags. For ClawHub skills the ref is a version.

Files and `--json` output always use the canonical form, for example `github.com/anthropics/skills//skills/skill-creator@main`. You can also pass a bare name such as `skill-creator` to `info`, `view`, `try` and `vendor`: New Tricks looks it up in the search index and asks you to pick when several skills share the name (in a script or with `--json` it fails and lists them).

## Inspect a skill with `info`

[`tricks info`](/tricks/reference/commands/info/) shows what you need to decide whether to use a skill: where it comes from, its licence and how that was detected, its risk surface, catalog signals, files and frontmatter. It fetches the skill into the local store; nothing runs.

```bash
tricks info anthropics/skills//skill-creator
```

```text
skill-creator  github.com/anthropics/skills//skills/skill-creator@main
  Create new skills, modify and improve existing skills, and measure skill performance. Use when …
  commit   33375500b (default-branch main)
  trust    official
  licence  Apache-2.0 [allow] via skill-file
  risk     10 script(s); 7 URL reference(s)
  installs 390.9k
  listed   clawhub, skills.sh
  files:
    LICENSE.txt                                          11345
    SKILL.md                                             33168
    agents/analyzer.md                                   10376
    …
    scripts/run_eval.py                                  11464  script
    scripts/run_loop.py                                  13605  script
    scripts/utils.py                                      1661  script
  frontmatter:
    name: skill-creator
    description: Create new skills, modify and improve existing skills, …
  content  `tricks view github.com/anthropics/skills//skills/skill-creator@main`
```

For catalog-hosted skills, `info` also prints the catalog's signals. For a ClawHub skill that includes downloads, stars, moderation, security status and the VirusTotal verdict:

```text
  licence  Proprietary [block] via frontmatter
  risk     2 URL reference(s)
  clawhub  downloads=49586 installs=1479 malware_blocked=false scanner_warnings=true security_status=clean stars=66 suspicious=false version=0.1.0 virustotal=suspicious
```

## Read a skill with `view`

[`tricks view`](/tricks/reference/commands/view/) prints `SKILL.md`, or any supporting file, exactly as it is. It does not render Markdown, so pipe it to a renderer or pager:

```bash
tricks view anthropics/skills//skill-creator | glow -
tricks view anthropics/skills//skill-creator references/schemas.md
tricks view anthropics/skills//skill-creator --json      # {skill, path, content}
```

Scripts are printed as text, never executed.

## Trust and verification

Skills are instructions for agents that can run commands, so New Tricks treats remote content as data and checks what it can:

- **Risk summary.** `info`, `vendor`, `outdated` and `publish` scan the files for scripts, `allowed-tools`, URL references, remote-execution patterns such as `curl … | sh`, hidden or bidirectional Unicode, and likely secrets. The same scanner powers lint's [NT5xx rules](/tricks/reference/lint-rules/).
- **Catalog security signals.** A Tessl security level of MEDIUM or above, a ClawHub suspicious or malware flag, a ClawHub moderation or security status other than clean, and a VirusTotal `malicious` verdict are added to the risk surface. VirusTotal `suspicious` is shown but not flagged, because it fires on any shell usage.
- **Verified downloads.** Git skills are addressed by commit and tree hash. ClawHub downloads are checked file by file against the version's published SHA-256 list: a mismatched, missing or unlisted file aborts the download, and a version without published hashes is refused. `.well-known` entries must carry a `sha256:` digest, which is verified, and archives cannot write outside the skill directory.
- **Licence.** `info` shows the licence class and where it came from (a licence file in the skill, frontmatter, the repository root, or the catalog's terms). When sources disagree, the most restrictive wins. See [Publishing](/tricks/concepts/publishing/#licence-gate) for what each class allows.

## Catalogs

Your catalogs live in the user config (`~/.config/newtricks/tricks.toml`). The first run writes that file with the recommended set, so nothing search uses is hidden:

```toml
[settings]
agents = ["claude"]      # agents to link skills for (a source repo can set its own)
fetch_interval = "24h"   # how often catalogs and upstreams are fetched again
live = ["skills.sh", "tessl", "clawhub", "github"]   # live-query catalogs asked on every search

[catalogs]
"github.com/anthropics/skills" = {}
"github.com/openai/skills" = {}
"github.com/vercel-labs/agent-skills" = {}
"github.com/github/awesome-copilot" = {}
"github.com/obra/superpowers" = {}
```

Manage them with [`tricks catalog`](/tricks/reference/commands/catalog/):

```bash
tricks catalog add acme/skills                         # a skill repository
tricks catalog add anthropics/claude-plugins-official --kind marketplace
tricks catalog add https://example.com                 # a site with /.well-known/agent-skills
tricks catalog add ./apm.yml                           # what a project already uses (read only)
tricks catalog list                                    # with skill counts and last indexed time
tricks catalog refresh [catalog]                       # re-index now
tricks catalog remove github.com/obra/superpowers
tricks catalog add --recommended                       # restore any recommended catalog you removed
```

The kind is detected when you omit `--kind`: a path to `apm.yml` or `skills-lock.json` is a pointer list, an `https://` URL outside GitHub is a `.well-known` site, and anything else is a repository. Pass `--kind marketplace` for a repository with a `marketplace.json`; a marketplace refresh indexes at most 60 of the repositories it points at. Private repositories work through your own GitHub credentials.

```text
github.com/anthropics/skills                                 repo            20 skills  indexed just now
github.com/github/awesome-copilot                            repo           441 skills  indexed just now
…
live: skills.sh, tessl, clawhub, github (GitHub code search needs `gh auth login`)
```

To stop asking a live catalog, remove it from `[settings] live`; to skip all of them for one search, pass `--no-live`.

## Next

- [Try a skill](/tricks/concepts/links-and-trials/) in a project before you commit to it.
- [Vendor it](/tricks/concepts/upstream/) into your source repo to customize it and keep merging upstream changes.
