//! Source repo commands: start, work, validate and ship.

use crate::cli::{Cmd, emit, short};
use crate::ctx::Ctx;
use crate::source_repo::{self, MergeReport, RepoStatus};
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
                    println!("next: `tricks vendor owner/repo//skill` or `tricks new my-skill`, then `tricks link`");
                }
            });
        }
        Cmd::New { name, description } => {
            let ws = source_repo::require(ctx)?;
            let r = source_repo::new_skill(&ws, &name, description.as_deref())?;
            emit(json, &r, |r| println!("created {} — edit {}/SKILL.md, then `tricks link {}`", r.name, r.path, r.name));
        }
        Cmd::Vendor { skill, name, path, upstream, base } => {
            let ws = source_repo::require(ctx)?;
            let o = source_repo::VendorOptions {
                name: name.as_deref(),
                path: path.as_deref(),
                upstream: upstream.as_deref(),
                base: base.as_deref(),
            };
            let r = source_repo::vendor(ctx, &ws, &skill, &o)?;
            emit(json, &r, print_vendor);
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
        Cmd::Edit { skill, branch, done } => {
            let r = if done { source_repo::edit_done(ctx, &skill)? } else { source_repo::edit(ctx, &skill, branch.as_deref())? };
            emit(json, &r, |r| {
                if r.vendored {
                    println!("vendored {} into the source repo", r.name);
                }
                println!("{}", r.path);
                if done {
                    eprintln!("finished editing `{}`; links deploy its active variant", r.name);
                } else if let Some(b) = &r.branch {
                    eprintln!("editing `{}` on branch {b}; commit there with git, then `tricks edit {} --done`", r.name, r.name);
                }
                for p in &r.placements {
                    eprintln!("  → {p}");
                }
            });
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
        Cmd::Merge { skill, dry_run, cont, abort } => {
            let ws = source_repo::require(ctx)?;
            let o = source_repo::MergeOptions { only: skill.as_deref(), dry_run, cont, abort };
            let r = source_repo::merge(ctx, &ws, &o)?;
            emit(json, &r, |r| if dry_run { print_merge_check(r) } else { print_merge(r) });
        }
        Cmd::Diff { skill, from, to } => {
            let ws = source_repo::require(ctx)?;
            let files = source_repo::changed_files(ctx, &ws, &skill, &from, &to)?;
            let mut out = Vec::new();
            for f in &files {
                let a = source_repo::version_file(ctx, &ws, &skill, &from, f)?.unwrap_or_default();
                let b = source_repo::version_file(ctx, &ws, &skill, &to, f)?.unwrap_or_default();
                let (sa, sb) = (String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
                let d = similar::TextDiff::from_lines(sa.as_ref(), sb.as_ref());
                out.push((f.clone(), d.unified_diff().header(&format!("{from}/{f}"), &format!("{to}/{f}")).to_string()));
            }
            emit(json, &out, |out| {
                if out.is_empty() {
                    println!("no differences between {from} and {to}");
                }
                for (_, d) in out {
                    print!("{d}");
                }
            });
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

#[derive(Debug, Serialize)]
pub struct RepoSummary {
    pub name: String,
    pub root: String,
    pub skills: usize,
}

#[derive(Debug, Serialize)]
pub struct Status {
    /// The source repo containing the current directory.
    pub source_repo: Option<RepoStatus>,
    pub links: Vec<crate::links::LinkInfo>,
    /// Registered source repos (from the user config).
    pub repos: Vec<RepoSummary>,
    /// Skills still deployed by New Tricks 0.2 user-scope installs.
    pub legacy: usize,
    pub unfinished_operations: Vec<String>,
}

pub fn status(ctx: &Ctx) -> Result<Status> {
    let source_repo = source_repo::current(ctx)?.map(|w| source_repo::status(ctx, &w)).transpose()?;
    Ok(Status {
        source_repo,
        links: crate::links::list(ctx)?,
        repos: source_repo::all_source_repos(ctx)?
            .into_iter()
            .map(|w| RepoSummary { name: w.name.clone(), root: w.root.to_string_lossy().to_string(), skills: w.manifest.skills.len() })
            .collect(),
        legacy: crate::links::legacy(ctx)?.len(),
        unfinished_operations: ctx.state.unfinished_ops()?.into_iter().map(|(_, op, d)| format!("{op} {d}")).collect(),
    })
}

pub fn print_status(s: &Status) {
    match &s.source_repo {
        Some(w) => print_repo_status(w),
        None if s.repos.is_empty() => println!("no source repos yet; run `tricks init` in a git repository"),
        None => {
            println!("source repos:");
            for r in &s.repos {
                println!("  {:<20} {:>3} skill(s)  {}", r.name, r.skills, r.root);
            }
        }
    }
    if !s.links.is_empty() {
        println!("links:");
        for l in &s.links {
            let h = if l.health == "ok" { String::new() } else { format!("  !! {}", l.health) };
            let skill = l.skill.strip_prefix("github.com/").unwrap_or(&l.skill);
            let skill = skill.rsplit_once("//").filter(|_| skill.starts_with("ws:")).map(|(_, n)| n).unwrap_or(skill);
            println!("  {:<8} {} → {} ({}, {}){}", l.agent, l.path, skill, l.kind, l.mode, h);
        }
    }
    if s.legacy > 0 {
        println!("{} skill(s) installed at user scope by New Tricks 0.2 are no longer managed; see `tricks doctor`", s.legacy);
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

fn print_merge(r: &MergeReport) {
    if r.items.is_empty() {
        println!("no vendored skills to merge");
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

fn print_merge_check(r: &MergeReport) {
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

pub fn print_repo_status(w: &RepoStatus) {
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
            notes.push("MERGE IN PROGRESS".into());
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
