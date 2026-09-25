//! Source repo commands: skills in the repo, work on them, upstream, validate and ship.

use crate::cli::{Cmd, emit, short};
use crate::ctx::Ctx;
use crate::source_repo::{self, RepoStatus, SourceRepo, SyncReport};
use anyhow::{Result, bail};
use serde::Serialize;

pub fn run(ctx: &Ctx, c: Cmd) -> Result<()> {
    let json = ctx.opts.json;
    match c {
        Cmd::Init { name, agent_skill } => {
            let r = source_repo::init(ctx, name.as_deref(), agent_skill)?;
            emit(json, &r, |r| {
                println!("{} source repo `{}` at {}", if r.created { "created" } else { "registered" }, r.name, r.root);
                for p in &r.agent_skill {
                    println!("  agent skill → {p}");
                }
                if r.created {
                    println!("next: `tricks create <name>` or `tricks vendor owner/repo//skill`, then `tricks link`");
                }
            });
        }
        Cmd::Create { name, description, from } => {
            let ws = source_repo::require(ctx)?;
            let r = source_repo::create(ctx, &ws, &name, description.as_deref(), from.as_deref())?;
            emit(json, &r, |r| {
                println!("created {} at {}", r.name, r.path);
                println!("  not committed yet; edit {}/SKILL.md, then `tricks link {}` to try it", r.path, r.name);
            });
        }
        Cmd::Vendor { skill, name, path, from, base } => {
            let ws = source_repo::require(ctx)?;
            let o =
                source_repo::VendorOptions { name: name.as_deref(), path: path.as_deref(), from: from.as_deref(), base: base.as_deref() };
            let r = source_repo::vendor(ctx, &ws, &skill, &o)?;
            emit(json, &r, print_vendor);
        }
        Cmd::Remove { skill } => {
            let ws = source_repo::require(ctx)?;
            let r = source_repo::remove(ctx, &ws, &skill)?;
            emit(json, &r, |r| {
                println!("removed {} ({}); not committed yet", r.name, r.path);
                for p in &r.unlinked {
                    println!("  unlinked {p}");
                }
            });
        }
        Cmd::Lint { skills, fix, strict } => {
            let one_path = skills.len() == 1 && std::path::Path::new(&skills[0]).join("SKILL.md").is_file();
            let ws = source_repo::current(ctx)?;
            let mut rep = match (&ws, one_path) {
                (Some(ws), false) => {
                    if fix {
                        let mut fixed = Vec::new();
                        for (n, s) in &ws.manifest.skills {
                            if skills.is_empty() || skills.contains(n) {
                                fixed.extend(crate::lint::fix_dir(&ws.root.join(&s.path))?.into_iter().map(|f| format!("{n}/{f}")));
                            }
                        }
                        let mut r = crate::lint::lint_repo_opts(ctx, ws, &skills, strict)?;
                        r.fixed = fixed;
                        r
                    } else {
                        crate::lint::lint_repo_opts(ctx, ws, &skills, strict)?
                    }
                }
                _ => {
                    let Some(p) = skills.first() else { bail!("not in a source repo: pass a skill directory") };
                    let dir = ctx.opts.cwd.join(p);
                    let fixed = if fix { crate::lint::fix_dir(&dir)? } else { vec![] };
                    let mut r = crate::lint::lint_path(&dir, strict);
                    r.fixed = fixed;
                    r
                }
            };
            rep.findings.retain(|f| f.severity != "info" || json);
            let errors = rep.errors;
            emit(json, &rep, |r| {
                for f in &r.fixed {
                    println!("fixed    {f}");
                }
                for f in &r.findings {
                    let loc = f.line.map(|l| format!(":{l}")).unwrap_or_default();
                    println!("{:<7} {} {}/{}{}  {}", f.severity, f.code, f.skill, f.file, loc, f.message);
                }
                println!("{} error(s), {} warning(s)", r.errors, r.warnings);
            });
            if errors > 0 {
                std::process::exit(1);
            }
        }
        Cmd::Edit { skill, branch, shell, commit, message, done } => {
            if commit {
                let r = source_repo::commit_draft(ctx, &skill, message.as_deref().unwrap_or_default())?;
                emit(json, &r, |r| match &r.commit {
                    Some(c) => println!(
                        "committed {} {} on {}{}",
                        r.name,
                        short(c),
                        r.branch,
                        r.agent.as_deref().map(|a| format!(" (Tricks-Agent: {a})")).unwrap_or_default()
                    ),
                    None => println!("nothing to commit for {} on {}", r.name, r.branch),
                });
                if !done {
                    return Ok(());
                }
            }
            let r = if done { source_repo::edit_done(ctx, &skill)? } else { source_repo::edit(ctx, &skill, branch.as_deref())? };
            emit(json, &r, |r| {
                println!("{}", r.path);
                if done {
                    eprintln!("finished editing `{}`; links deploy its active variant", r.name);
                } else if let Some(b) = &r.branch {
                    eprintln!(
                        "editing `{}` on branch {b} (`cd \"$(tricks edit {})\"` or `--shell` to work there); commit drafts with `tricks edit {} --commit -m \"…\"`, then `tricks merge {}@{b}`",
                        r.name, r.name, r.name, r.name
                    );
                }
                for p in &r.placements {
                    eprintln!("  → {p}");
                }
            });
            if shell {
                open_shell(&r.path, &format!("{}@{}", r.name, r.branch.as_deref().unwrap_or_default()))?;
            }
        }
        Cmd::Use { spec, local, reset } => {
            let r = source_repo::use_variant(ctx, &spec, local, reset)?;
            emit(json, &r, |r| {
                println!(
                    "{} now uses {}{}",
                    r.name,
                    r.variant.as_deref().unwrap_or("the main checkout (dev)"),
                    if r.local { " (local override)" } else { "" }
                );
                for p in &r.placements {
                    println!("  → {p}");
                }
            });
        }
        Cmd::Diff { skill, range } => {
            let ws = source_repo::require(ctx)?;
            let (from, to) = parse_range(range.as_deref())?;
            let out = unified(ctx, &ws, &skill, &from, &to)?;
            emit(json, &out, |out| {
                if out.is_empty() {
                    println!("no differences between {from} and {to}");
                }
                for (_, d) in out {
                    print!("{d}");
                }
            });
        }
        Cmd::Merge { spec, whole_branch, pr, message } => {
            let o = source_repo::MergeOptions { whole_branch, pr, message: message.as_deref() };
            let r = source_repo::merge_branch(ctx, &spec, &o)?;
            emit(json, &r, |r| {
                let what = if r.mode == "branch" { format!("branch {}", r.branch) } else { format!("{} from {}", r.name, r.branch) };
                if let Some(u) = &r.pr_url {
                    println!("pull request for {what} into {}: {u}", r.into);
                } else if !r.conflicts.is_empty() {
                    println!("merging {what} into {} stopped on conflicts:", r.into);
                    for c in &r.conflicts {
                        println!("    CONFLICT {c}");
                    }
                    println!("  resolve them, then commit with git{}", if r.mode == "branch" { " (`git merge --continue`)" } else { "" });
                } else if let Some(c) = &r.commit {
                    println!("merged {what} into {} ({})", r.into, short(c));
                    for p in &r.placements {
                        println!("  → {p}");
                    }
                }
                if !r.other_paths.is_empty() {
                    println!(
                        "  note: {} also changes {} file(s) outside {} (not merged; use --whole-branch to include them)",
                        r.branch,
                        r.other_paths.len(),
                        r.name
                    );
                }
            });
        }
        Cmd::Outdated { skill, diff } => {
            let ws = source_repo::require(ctx)?;
            let r = source_repo::outdated(ctx, &ws, skill.as_deref())?;
            let diffs = if diff { diffs_for(ctx, &ws, &r, "base", "upstream") } else { vec![] };
            if json {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "items": r.items, "diffs": diffs }))?);
            } else {
                print_outdated(&r);
                print_diffs(&diffs);
            }
        }
        Cmd::Sync { skill, dry_run, cont, abort } => {
            let ws = source_repo::require(ctx)?;
            let o = source_repo::SyncOptions { only: skill.as_deref(), dry_run, cont, abort };
            let r = source_repo::sync(ctx, &ws, &o)?;
            if dry_run {
                let diffs = diffs_for(ctx, &ws, &r, "working", "candidate");
                if json {
                    println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "items": r.items, "diffs": diffs }))?);
                } else {
                    print_outdated(&r);
                    print_diffs(&diffs);
                }
            } else {
                emit(json, &r, print_sync);
            }
        }
        Cmd::Contribute { skill, title, body, dry_run } => {
            let r = crate::contribute::contribute(ctx, &skill, title.as_deref(), body.as_deref(), dry_run)?;
            emit(json, &r, |r| {
                println!("{} → {} (branch {})", r.skill, r.upstream, r.branch);
                println!("{}", r.diffstat);
                match &r.url {
                    Some(u) => println!("opened {u}"),
                    None => println!("dry run: prepared in {}", r.worktree),
                }
            });
        }
        Cmd::Publish { target, bump, dry_run, push, pr, accept_copyleft } => {
            let r = crate::publish::publish(ctx, &crate::publish::PublishOptions { target, bump, dry_run, push, pr, accept_copyleft })?;
            let blocked = r.blocked;
            emit(json, &r, print_publish);
            if blocked {
                std::process::exit(1);
            }
        }
        _ => unreachable!("handled in cli::run"),
    }
    Ok(())
}

/// `edit --shell`: an interactive shell in the draft; `exit` returns.
fn open_shell(dir: &str, what: &str) -> Result<()> {
    let shell = if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    };
    eprintln!("starting {shell} in {dir} (`exit` to return)");
    let status = std::process::Command::new(&shell).current_dir(dir).env("TRICKS_EDITING", what).status()?;
    if !status.success() {
        eprintln!("shell exited with {status}");
    }
    Ok(())
}

/// `a..b`, `a..` (to working), `..b` (from head), `a` (a..working); default head..working.
fn parse_range(range: Option<&str>) -> Result<(String, String)> {
    let (a, b) = match range {
        None => ("", ""),
        Some(r) => match r.split_once("..") {
            Some((a, b)) => (a, b),
            None => (r, ""),
        },
    };
    let a = if a.is_empty() { "head" } else { a };
    let b = if b.is_empty() { "working" } else { b };
    for v in [a, b] {
        match v {
            "upstream" | "U" => bail!("compare with upstream using `tricks outdated <skill> --diff`"),
            "candidate" | "R" => bail!("preview the upstream sync with `tricks sync <skill> --dry-run`"),
            _ => {}
        }
    }
    Ok((a.to_string(), b.to_string()))
}

/// (file, unified diff) for every file that differs between two versions of a skill.
fn unified(ctx: &Ctx, ws: &SourceRepo, skill: &str, from: &str, to: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for f in source_repo::changed_files(ctx, ws, skill, from, to)? {
        let a = source_repo::version_file(ctx, ws, skill, from, &f)?.unwrap_or_default();
        let b = source_repo::version_file(ctx, ws, skill, to, &f)?.unwrap_or_default();
        let (sa, sb) = (String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
        let d = similar::TextDiff::from_lines(sa.as_ref(), sb.as_ref());
        out.push((f.clone(), d.unified_diff().header(&format!("{from}/{skill}/{f}"), &format!("{to}/{skill}/{f}")).to_string()));
    }
    Ok(out)
}

fn diffs_for(ctx: &Ctx, ws: &SourceRepo, r: &SyncReport, from: &str, to: &str) -> Vec<(String, Vec<(String, String)>)> {
    r.items
        .iter()
        .filter(|i| i.state == "update-available")
        .map(|i| (i.name.clone(), unified(ctx, ws, &i.name, from, to).unwrap_or_else(|e| vec![(String::new(), format!("error: {e:#}\n"))])))
        .collect()
}

fn print_diffs(diffs: &[(String, Vec<(String, String)>)]) {
    for (_, files) in diffs {
        for (_, d) in files {
            print!("{d}");
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RepoSummary {
    pub name: String,
    pub root: String,
    pub skills: usize,
}

#[derive(Debug, Serialize)]
pub struct List {
    /// The source repo containing the current directory.
    pub source_repo: Option<RepoStatus>,
    /// Registered source repos (from the user config).
    pub repos: Vec<RepoSummary>,
    /// Links of this source repo's skills (every source repo's with `--all`).
    pub links: Vec<crate::links::LinkInfo>,
    /// Trials in this project and at user level (everywhere with `--all`).
    pub trials: Vec<crate::links::LinkInfo>,
    pub unfinished_operations: Vec<String>,
}

pub fn list(ctx: &Ctx, all: bool) -> Result<List> {
    let ws = source_repo::current(ctx)?;
    let links = match (&ws, all) {
        (_, true) => crate::links::dev_links(ctx, None)?,
        (Some(w), false) => crate::links::dev_links(ctx, Some(w))?,
        (None, false) => vec![],
    };
    Ok(List {
        source_repo: ws.as_ref().map(|w| source_repo::status(ctx, w)).transpose()?,
        repos: source_repo::all_source_repos(ctx)?
            .into_iter()
            .map(|w| RepoSummary { name: w.name.clone(), root: w.root.to_string_lossy().to_string(), skills: w.manifest.skills.len() })
            .collect(),
        links,
        trials: crate::links::trials(ctx, all)?,
        unfinished_operations: ctx.state.unfinished_ops()?.into_iter().map(|(_, op, d)| format!("{op} {d}")).collect(),
    })
}

fn place_label(scope: &str) -> String {
    if scope == "global" { "user level".into() } else { scope.to_string() }
}

/// Links grouped by where they are: user level first, then each project.
fn print_grouped(items: &[crate::links::LinkInfo], dev: bool) {
    let mut scopes: Vec<&str> = items.iter().map(|l| l.scope.as_str()).collect();
    scopes.sort_by_key(|s| (*s != "global", s.to_string()));
    scopes.dedup();
    for scope in scopes {
        println!("{}:", place_label(scope));
        for l in items.iter().filter(|l| l.scope == scope) {
            let h = if l.health == "ok" { String::new() } else { format!("  !! {}", l.health) };
            let what = if dev { l.name.clone() } else { l.skill.strip_prefix("github.com/").unwrap_or(&l.skill).to_string() };
            println!("  {what:<28} {:<8} {} ({}){h}", l.agent, l.path, l.mode);
        }
    }
}

pub fn print_list(s: &List, links: bool, trials: bool) {
    if links {
        if s.links.is_empty() {
            println!("no links; `tricks link` links this source repo's skills for your agents");
        }
        print_grouped(&s.links, true);
        return;
    }
    if trials {
        if s.trials.is_empty() {
            println!("no trials here; `tricks try <skill>` tries a skill from elsewhere (`--all` lists every project's)");
        }
        print_grouped(&s.trials, false);
        return;
    }
    match &s.source_repo {
        Some(w) => print_repo_status(w, &s.links),
        None if s.repos.is_empty() => println!("no source repos yet; run `tricks init` in a git repository"),
        None => {
            println!("source repos:");
            for r in &s.repos {
                println!("  {:<20} {:>3} skill(s)  {}", r.name, r.skills, r.root);
            }
        }
    }
    if !s.trials.is_empty() {
        println!("{} trial(s) here; see `tricks list --trials`", s.trials.len());
    }
    for u in &s.unfinished_operations {
        println!("interrupted operation: {u} (re-run the command to recover)");
    }
}

fn print_vendor(r: &source_repo::VendorReport) {
    println!("added {} at {}", r.name, r.path);
    if let Some(u) = &r.upstream {
        println!("  upstream {u} @ {}", r.base.as_deref().map(short).unwrap_or(""));
    }
    if let Some(l) = &r.license {
        println!("  licence  {} [{}]", l.spdx.as_deref().unwrap_or("none"), l.class);
    }
    if !r.risk.is_empty() {
        println!("  risk     {}", r.risk.join("; "));
    }
    println!("  not committed yet: review and `git commit` when ready; `tricks link {}` to try it", r.name);
}

fn print_sync(r: &SyncReport) {
    if r.items.is_empty() {
        println!("no vendored skills to sync");
    }
    for i in &r.items {
        let to = i.to_ref.clone().or(i.to.as_deref().map(|c| short(c).to_string())).unwrap_or_default();
        println!("{:<12} {} {}", i.state, i.name, if to.is_empty() { String::new() } else { format!("→ {to}") });
        for f in &i.incoming {
            println!("    incoming {f}");
        }
        if let Some(o) = &i.outcome {
            for f in &o.merged {
                println!("    merged   {f}");
            }
            for f in o.updated.iter().chain(o.added.iter()) {
                println!("    updated  {f}");
            }
            for f in &o.deleted {
                println!("    deleted  {f}");
            }
            for c in &o.conflicts {
                println!("    CONFLICT {} ({})", c.path, c.kind);
            }
        }
        for r in &i.risk {
            println!("    risk     {r}");
        }
        if let Some(m) = &i.message {
            println!("    {m}");
        }
    }
}

fn print_outdated(r: &SyncReport) {
    let mut any = false;
    for i in &r.items {
        if i.state == "up-to-date" {
            continue;
        }
        any = true;
        println!("{:<24} {} → {}", i.name, i.from.as_deref().map(short).unwrap_or("?"), i.to_ref.clone().unwrap_or_default());
        for f in &i.incoming {
            println!("    {f}");
        }
        for r in &i.risk {
            println!("    risk {r}");
        }
        if let Some(m) = &i.message {
            println!("    {m}");
        }
    }
    if !any {
        println!("all vendored skills are up to date");
    }
}

pub fn print_repo_status(w: &RepoStatus, links: &[crate::links::LinkInfo]) {
    println!("source repo {} ({}{})", w.name, w.root, w.branch.as_deref().map(|b| format!(", branch {b}")).unwrap_or_default());
    for s in &w.skills {
        let mut notes = Vec::new();
        match &s.upstream {
            Some(u) => notes.push(format!("from {}", u.strip_prefix("github.com/").unwrap_or(u))),
            None => notes.push("original".into()),
        }
        if s.customized {
            notes.push("customized".into());
        }
        if let Some(u) = &s.update_available {
            notes.push(format!("upstream has {u}"));
        }
        if s.merge_in_progress {
            notes.push("SYNC IN PROGRESS".into());
        }
        if s.uncommitted {
            notes.push("uncommitted".into());
        }
        if s.lint_errors + s.lint_warnings > 0 {
            notes.push(format!("lint {}E/{}W", s.lint_errors, s.lint_warnings));
        }
        if let Some(v) = &s.variant {
            notes.push(format!("using {v}"));
        }
        if let Some(e) = &s.editing {
            notes.push(format!("editing on {e}"));
        }
        if !s.branches.is_empty() {
            notes.push(format!("branches: {}", s.branches.join(", ")));
        }
        let key = format!("ws:{}//{}", w.root, s.name);
        let mut places: Vec<String> = links.iter().filter(|l| l.skill == key).map(|l| place_label(&l.scope)).collect();
        places.sort();
        places.dedup();
        if !places.is_empty() {
            notes.push(format!("linked: {}", places.join(", ")));
        }
        println!("  {:<22} {}", s.name, notes.join(" · "));
    }
    if !w.targets.is_empty() {
        println!("  publish targets: {}", w.targets.join(", "));
    }
}

fn print_publish(r: &crate::publish::PublishReport) {
    println!("publish {} → {}{}", r.target, r.repo, if r.dry_run { " (dry run)" } else { "" });
    for g in &r.gates {
        let icon = match g.status.as_str() {
            "pass" => "✓",
            "warn" => "!",
            _ => "✗",
        };
        println!("  {icon} {}", g.name);
        for d in &g.details {
            println!("      {d}");
        }
    }
    println!(
        "  version: {} → {}  (suggested bump: {})",
        r.previous_version.as_deref().unwrap_or("none"),
        r.version.as_deref().unwrap_or("untagged"),
        r.suggested_bump.as_deref().unwrap_or("-")
    );
    if r.changes.is_empty() {
        println!("  no changes to publish");
    } else {
        println!("  changes:");
        for c in r.changes.iter().take(40) {
            println!("    {c}");
        }
        if r.changes.len() > 40 {
            println!("    … {} more", r.changes.len() - 40);
        }
    }
    if r.blocked {
        println!("blocked: fix the failing gates above");
    }
    if let Some(c) = &r.commit {
        println!("committed {}{}", short(c), r.tag.as_deref().map(|t| format!(", tagged {t}")).unwrap_or_default());
    }
    if r.pushed {
        println!("pushed");
    }
    if let Some(u) = &r.pr_url {
        println!("pull request: {u}");
    }
}
