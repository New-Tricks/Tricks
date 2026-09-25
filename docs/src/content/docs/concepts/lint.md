---
title: Lint
description: Check skills against the Agent Skills spec and for structure, triggering, compatibility and safety problems, with stable rule codes you can configure.
---

A skill that parses is not necessarily a skill that works. [`tricks lint`](/tricks/reference/commands/lint/) checks the things that make agents skip, misfire on, or mistrust a skill: spec violations, broken links, vague descriptions, keys an agent won't understand, and unsafe content. Rules have stable codes, so you can configure them and grep for them.

## Run it

```bash
tricks lint                        # every skill in the source repo
tricks lint changelog-writer       # one or more skills by name
tricks lint ~/drafts/my-skill      # any skill directory, even outside a source repo
```

```text
error   NT102 release-notes/SKILL.md:2  `Release-Notes` must be lowercase
error   NT103 release-notes/SKILL.md:2  name `Release-Notes` ≠ folder `release-notes`
warning NT301 release-notes/SKILL.md:3  say when the skill should be used, e.g. "Use when …"
warning NT302 release-notes/SKILL.md:3  26 characters; agents trigger on keywords in the description
warning NT305 release-notes/SKILL.md:3  write descriptions in the third person ("Extracts…", not "I can…")
warning NT504 release-notes/SKILL.md:4  `Bash(*)`: scope tools narrowly, e.g. Bash(git:*)
error   NT201 release-notes/SKILL.md:9  `references/template.md` does not exist
error   NT202 release-notes/SKILL.md:10  `scripts/collect.sh` does not exist
warning NT206 skill-creator/SKILL.md  ~8232 tokens; the whole body loads on activation
warning NT203 skill-creator/SKILL.md:373  `~/Downloads/eval_set.json` will not exist on other machines
4 error(s), 6 warning(s)
```

Each line gives the severity, the rule code, the skill, file and line, and what to do. `lint` exits with status 1 when there is at least one error, so it works as a CI step. Findings with severity `info` are left out of the text output; `--json` includes them.

## What it checks

Rules come in five families. The full list, with default severities, is in the [lint rule reference](/tricks/reference/lint-rules/).

| Family | Checks | Default |
|---|---|---|
| **NT1xx** Spec conformance | valid frontmatter; `name` present, well formed and equal to the folder name; `description` present and at most 1024 characters; `compatibility` at most 500; `metadata` a map of strings; `skill.md` instead of `SKILL.md` | error (NT110 warning) |
| **NT2xx** Structure | broken relative links; referenced `scripts/…` files that don't exist; absolute or `~/` paths; references nested more than one level; `SKILL.md` over 500 lines; body over about 5,000 tokens | error for broken links and missing scripts, warning otherwise |
| **NT3xx** Triggering | description without a "use when …" clause; description under 60 characters; first- or second-person description; the same `name` twice in the source repo; two descriptions so similar they compete for triggering | warning; duplicate name is an error |
| **NT4xx** Agent compatibility | Claude Code-only keys (`model`, `hooks`, `argument-hint`, …) when Claude is not one of the repo's agents; keys outside the spec; non-ASCII frontmatter, which APM rejects | warning; unknown keys are info |
| **NT5xx** Safety | hidden or bidirectional Unicode; likely secrets; scripts that download and run remote code (`curl … \| sh`); broad `allowed-tools` such as `Bash(*)` | error for Unicode and secrets, warning otherwise |

The agents NT401 checks against are the source repo's `agents` (or your user setting); see [Agents](/tricks/concepts/agents/). When you lint a directory outside a source repo, all four agents count as targeted.

## Fix what can be fixed mechanically

```bash
tricks lint --fix
```

```text
fixed    release-notes/SKILL.md: name `Release-Notes` → `release-notes`
```

`--fix` makes only changes that have exactly one right answer:

- renames `skill.md` to `SKILL.md`;
- lowercases `name` when the lowercase form is the folder name;
- removes trailing whitespace and converts CRLF line endings to LF in `.md`, `.txt`, `.yaml` and `.yml` files.

It never rewrites prose: it won't touch a description, rename a folder, add a missing file or change `allowed-tools`. Everything else is reported for you to decide. The fixed files are left uncommitted.

:::caution
`--fix` applies to vendored skills too. Whitespace fixes in an upstream copy become part of your customization and can cause conflicts on the next [`update`](/tricks/concepts/upstream/). Name the skills you want fixed if you'd rather leave vendored ones alone.
:::

## Configure rules

Lint configuration lives under `[lint]` in the source repo's `tricks.toml`:

```toml
[lint]
ignore = ["NT206"]            # rules to skip everywhere in this repo
strict-spec = false           # true: keys outside the Agent Skills spec are errors

[lint.per-skill.release-notes]
ignore = ["NT504"]            # rules to skip for one skill (the [skills.<name>] key)
```

Rules can be ignored but their severity can't be changed. An ignored rule produces no finding at all, including in `--json` and the publish gate.

### Inline disables

A skill can switch rules off for itself in its frontmatter, which travels with the skill wherever you copy it:

```yaml
---
name: release-notes
description: Drafts release notes from merged pull requests. Use when …
metadata:
  tricks-lint-disable: NT301, NT302
---
```

Separate codes with commas or spaces. `publish` strips every `tricks-*` metadata key, so consumers never see it, and so does `contribute` when it offers a change upstream. Inline disables don't cover the repo-wide checks for duplicate names (NT303) and competing descriptions (NT304); use `[lint]` or `[lint.per-skill.<name>]` for those.

## Strict spec mode

The Agent Skills spec defines six frontmatter keys: `name`, `description`, `license`, `compatibility`, `metadata` and `allowed-tools`. Agents add their own (Claude Code has `model`, `hooks`, `argument-hint` and more), and by default New Tricks reports an unknown key as info (NT402) and preserves it. Strict mode makes any key outside the spec an error, which is what the reference validator does:

```bash
tricks lint --strict
```

```text
error   NT402 release-notes/SKILL.md:5  `tags` is not in the Agent Skills spec (strict-spec)
```

Set `strict-spec = true` under `[lint]` to make it the default for the repo, including the publish gate.

## Conformance with `skills-ref`

The NT1xx rules implement the [Agent Skills specification](https://agentskills.io/specification) natively and are tested against every validator and parser case of the official `skills-ref` reference validator: name format (lowercase letters of any script, digits and single hyphens, NFKC-normalized), name matching the folder, description and compatibility limits, frontmatter parsing.

Two differences are deliberate:

- **Unknown keys** are info (NT402), not errors, unless you use `--strict` or `strict-spec`. Agent-specific keys are common and legitimate; NT401 already warns about the ones your agents won't read.
- **`skill.md`** is accepted, as `skills-ref` accepts it, but with a warning (NT110), because Claude Code only loads the uppercase `SKILL.md`. `--fix` renames it.

## JSON output

`--json` gives every finding, including info, with a `fixable` flag:

```bash
tricks lint --json
```

```json
{
  "findings": [
    {
      "code": "NT203",
      "severity": "warning",
      "skill": "skill-creator",
      "file": "SKILL.md",
      "line": 373,
      "message": "`~/Downloads/eval_set.json` will not exist on other machines",
      "fixable": false
    }
  ],
  "errors": 0,
  "warnings": 1,
  "fixed": []
}
```

The VS Code extension shows the same findings in the Problems panel.

## Lint as a publish gate

[`tricks publish`](/tricks/concepts/publishing/) runs lint on the skills it publishes and refuses to publish while there is any error. Warnings are listed in the pre-flight but don't block. Your `ignore`, per-skill and inline settings apply, and `strict-spec = true` makes unknown keys block publishing too.
