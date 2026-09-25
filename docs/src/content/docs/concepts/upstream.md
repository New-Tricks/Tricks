---
title: Upstream tracking
description: Vendor an upstream skill into your source repo, customize it, and keep merging upstream improvements into your version with a three-way merge.
---

Most good skills start as someone else's. Vendoring copies an upstream skill into your [source repo](/tricks/concepts/source-repo/) and records where it came from and which revision you started from. You then change it freely, and when upstream improves, [`tricks update`](/tricks/reference/commands/update/) merges their changes into yours, the way `git merge` would, keeping your customizations.

Upstream changes never reach a vendored skill on their own. They are reported, you review them, and you merge them on purpose.

## Vendor a skill

```bash
cd ~/code/my-skills
tricks vendor anthropics/skills//skill-creator
```

```text
added skill-creator at skills/skill-creator
  upstream github.com/anthropics/skills//skills/skill-creator @ 079fa83de
  licence  Apache-2.0 [allow]
  risk     10 script(s); 7 URL reference(s)
  not committed yet: review and `git commit` when ready; `tricks link skill-creator` to try it
```

[`vendor`](/tricks/reference/commands/vendor/) accepts any [skill identifier](/tricks/concepts/discovery/#skill-identifiers), including GitHub URLs and catalog-hosted skills. `--name` changes the skill's name in your repo and `--path` its location (default `skills/<name>`). The copy is left uncommitted: look it over, then commit it with git.

It records the intent in `tricks.toml`:

```toml
[skills.skill-creator]
path = "skills/skill-creator"
upstream = "github.com/anthropics/skills//skills/skill-creator"
track = "latest"
update = "review"
```

and the resolved facts in `tricks.lock`, which New Tricks maintains:

```toml
[skills.skill-creator]
base = "079fa83de1ea6fb2c6c345fd842392aac3bc4d35"
base_tree = "3cf9a8db32597ba3e24b584a3d696f4e11c7d7b6"

[skills.skill-creator.license]
spdx = "Apache-2.0"
class = "allow"
source = "skill-file"
confidence = 1.0
```

- `upstream` is the canonical identifier of the skill you copied.
- `track` is what "latest upstream" means for this skill: `latest` (the highest semver tag, or the default branch when there are none), a branch name, or a version. Vendoring from an explicit branch (`…//skill-creator@main`) tracks that branch; anything else tracks `latest`.
- `update` is the [update policy](#update-policies).
- `base` is the upstream commit your copy last incorporated, and `base_tree` the hash of the skill directory at that commit.

### A copy you made earlier

If you already copied a skill by hand, tell New Tricks which upstream revision it started from, and future updates merge from there:

```bash
tricks vendor anthropics/skills//skill-creator --from ~/old/skill-creator --base 079fa83
```

`--base` takes a commit, a tag, or a catalog version. `.well-known` sites serve only their latest revision, so `--base` is refused for them.

### Licence check

`vendor` detects the upstream licence. For the `block` class (no licence, proprietary or unknown terms, no-derivatives licences) and `non-commercial`, it asks for confirmation, because the terms may prohibit modification or redistribution:

```text
error: confirmation required: Vendor github.com/anthropics/skills//skills/pdf@main anyway? (re-run with --yes to confirm)
  licence: Proprietary (block, via skill-file)
  its terms may prohibit modification or redistribution; you are responsible for complying
```

Vendoring such a skill for private use is your call. Publishing it is blocked unless you record a reason in `tricks.toml` with `license-override`; see [Publishing](/tricks/concepts/publishing/#licence-gate).

## Four versions: B, U, C and R

Every upstream operation works with four versions of the skill:

| | Version | Where it lives |
|---|---|---|
| **B** | Base: the upstream revision you last incorporated | `base` in `tricks.lock` |
| **U** | Upstream: the latest revision of what you track | fetched into New Tricks' mirror of the upstream |
| **C** | Yours: the customized copy in your source repo | `skills/<name>/` |
| **R** | Result: C with the changes from B to U merged in | your working tree after `update` |

Each question you might ask maps to one comparison:

| Question | Compare | Command |
|---|---|---|
| What did I change? | B → C | `tricks diff <skill> base..` |
| What did upstream change? | B → U | `tricks outdated <skill> --diff` |
| What will change for me? | C → R | `tricks update <skill> --dry-run` |

## See what changed upstream

[`tricks outdated`](/tricks/reference/commands/outdated/) fetches upstreams (throttled by `fetch_interval`, default 24 hours; `--offline` uses what is cached) and reports incoming changes with a risk summary. It changes nothing.

```bash
tricks outdated
```

```text
skill-creator            079fa83de → main
    M SKILL.md
    A scripts/check_links.py
    risk + script scripts/check_links.py
    risk +1 URL(s): https://agentskills.io/specification
```

The risk lines call out what makes a change worth a closer look: new or removed scripts, widened `allowed-tools`, new URLs, remote-execution patterns, hidden Unicode and possible secrets. A clean text merge says nothing about behaviour, so read these before you merge.

Add `--diff` to see the incoming change itself (B → U):

```text
--- base/skill-creator/SKILL.md
+++ upstream/skill-creator/SKILL.md
@@ -5,7 +5,7 @@

 # Skill Creator

-A skill for creating new skills and iteratively improving them.
+A skill for creating new skills, validating them, and iteratively improving them.
```

[`tricks list`](/tricks/reference/commands/list/) also shows `upstream has main` next to a skill with changes waiting, but it reads only cached state and never fetches.

## Merge upstream changes with `update`

Preview the result first. `--dry-run` shows the diff from your current version to the merge result (C → R) and changes nothing:

```bash
tricks update skill-creator --dry-run
```

Then merge:

```bash
tricks update skill-creator
```

```text
merged       skill-creator → main
    incoming M SKILL.md
    incoming A scripts/check_links.py
    merged   SKILL.md
    updated  scripts/check_links.py
    risk     + script scripts/check_links.py
    risk     +1 URL(s): https://agentskills.io/specification
    merged into the working tree (uncommitted); review with `git diff` and commit — agents keep the previous version until then
```

`update` three-way merges B → U into your copy and moves `base` in `tricks.lock` to the new upstream revision. The result is left **uncommitted**: review it with `git diff` or your editor's source control view, then commit it with git. Without a skill name, `update` merges every vendored skill whose policy is `review`.

The skill's folder must have no uncommitted changes when you start; otherwise `update` stops and asks you to commit or stash them.

### Links stay on the committed version

If the skill is [linked](/tricks/concepts/links-and-trials/) for testing, agents keep loading the committed version while the merge is in your working tree. The link moves to a snapshot of your last commit, and `list --links` shows it:

```text
~/code/my-app:
  skill-creator                claude   ~/code/my-app/.claude/skills/skill-creator (link)  main @ ec630c3 (snapshot)
```

Once you commit the merged result (or abort), the next `tricks` command returns the link to your live working tree:

```text
  skill-creator                claude   ~/code/my-app/.claude/skills/skill-creator (link)  main (working tree, live)
```

### Conflicts

When upstream changed the same lines you did, `update` stops with standard conflict markers:

```text
conflicts    skill-creator → main
    incoming M SKILL.md
    CONFLICT SKILL.md (text)
    resolve the conflicts, then run `tricks update --continue` (or `--abort`)
```

```text
<<<<<<< skill-creator (yours)
Before you start, read [our house style](references/house-style.md): it covers naming, tone and the review checklist our team uses.
=======
Ready? Start by asking the user what the skill should do.
>>>>>>> upstream main
```

Edit the file to the version you want (the VS Code extension opens it in the three-way merge editor), then:

```bash
tricks update --continue          # checks no markers remain, records the new base
git commit -am "Merge upstream skill-creator"
```

`--continue` refuses while markers remain (`unresolved conflicts remain: SKILL.md (conflict markers)`). `tricks update --abort` restores your version exactly as it was before the update. `update` exits with status 1 when it stops on conflicts or a skill fails (a `--dry-run` that finds conflicts doesn't), so scripts and CI notice. Only one upstream update can be in progress per source repo; `tricks list` marks it `UPDATE IN PROGRESS`.

Conflicts that markers can't express are reported by kind:

| Kind | What happened | What you do |
|---|---|---|
| `text` | both sides changed the same lines | edit out the markers |
| `added-both` | both sides added a file with the same name | edit out the markers |
| `binary` | both sides changed a binary file | upstream's copy is written beside yours as `<file>.upstream`; keep one and delete the `.upstream` file |
| `deleted-locally` | you deleted a file upstream changed | upstream's copy is written as `<file>.upstream`; restore it or delete it |
| `deleted-upstream` | upstream deleted a file you changed | your file is kept; delete it if you agree |

`--continue` also refuses while an `.upstream` file is left over. Publishing never ships `.upstream` files.

## Update policies

Set `update` per vendored skill in `tricks.toml`:

| `update =` | `outdated` | `update` (no skill named) | `update <skill>` |
|---|---|---|---|
| `review` (default) | reports changes | merges | merges |
| `pinned` | reports changes with `pinned: name it (tricks update <skill>) to take the new version` | skips | merges |
| `paused` | not checked | skips | merges |

Use `pinned` for a skill you want to stay on a known revision but still hear about, and `paused` for one you have diverged from for good.

## Renamed or removed upstream

If upstream moves the skill to another path, `update` follows the rename, prints `upstream moved <skill> to <path>; recorded in tricks.lock`, and records the new path as `upstream_path` in the lock. If upstream deletes the skill, `outdated` reports `upstream no longer contains … (deleted or moved); the vendored copy is kept`, and your copy stays as it is.

## Catalog-hosted upstreams

Skills hosted by a catalog rather than git work the same way:

```bash
tricks vendor clawhub.ai/awspace/skills//pdf
tricks vendor example.com/.well-known/agent-skills//invoice
```

- **ClawHub** skills record `clawhub:<version>` as their base. Every download is verified file by file against the version's published SHA-256 list. ClawHub skills that ClawHub serves from GitHub are vendored as the git skill they point to.
- **`.well-known`** skills record the entry's `sha256:` digest as their base. `update` re-reads the site's index first so a new digest is seen.

The base snapshot is kept in New Tricks' store so the three-way merge has a B to work from. ClawHub can serve an old version again if the snapshot is gone; a `.well-known` site cannot, and New Tricks then asks you to vendor the skill again.

## See your customization

```bash
tricks diff skill-creator base..
```

```text
--- base/skill-creator/SKILL.md
+++ working/skill-creator/SKILL.md
@@ -29,6 +29,8 @@

 Ready? Start by asking the user what the skill should do.

+Before you start, read [our house style](references/house-style.md): it covers naming, tone and the review checklist our team uses.
+
 ## Communicating with the user
--- base/skill-creator/references/house-style.md
+++ working/skill-creator/references/house-style.md
@@ -0,0 +1,5 @@
+# House style
…
```

`base..` is short for `base..working`. [`tricks diff`](/tricks/reference/commands/diff/) also compares `head`, `working`, branches and commits; see [Branch experiments](/tricks/concepts/branch-experiments/#compare-versions).

## Offer your change upstream

If your customization would help everyone, [`tricks contribute`](/tricks/reference/commands/contribute/) turns it into a pull request on the upstream repository:

```bash
tricks contribute skill-creator --dry-run
```

```text
skill-creator → github.com/anthropics/skills (branch tricks/skill-creator-1790368223)
 skills/skill-creator/SKILL.md                  | 2 ++
 skills/skill-creator/references/house-style.md | 5 +++++
 2 files changed, 7 insertions(+)
dry run: prepared in …/work/pr/cced1a279781-skill-creator-1790368223
```

It takes only your B → C change for that one skill, strips New Tricks' own `tricks-*` metadata, and applies it onto upstream's current default branch. Without `--dry-run` it shows which files become public, asks for confirmation, forks the upstream with `gh`, pushes the branch and opens the pull request (`--title`, `--body` to set them). Nothing else from your source repo leaves it. If your change conflicts with current upstream, run `tricks update` first. Pull requests work for GitHub upstreams only.
