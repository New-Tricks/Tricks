//! Workspace, experiment and publish commands.

use crate::cli::{emit, short};
use crate::ctx::Ctx;
use crate::workspace::{self, WsInstallReport, WsStatus, WsUpdateReport};
use anyhow::{Result, bail};
use clap::Subcommand;

#[derive(Subcommand)]
pub enum WsCmd {
    /// Make the current git repository a skills workspace
    Init {
        #[arg(long)]
        name: Option<String>,
        /// Also install the bundled `tricks` agent skill
        #[arg(long)]
        agent_skill: bool,
    },
    /// Copy an upstream skill into the workspace to customize it
    Vendor {
        skill: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        path: Option<String>,
    },
    /// Import a skill from a local folder
    Import {
        folder: String,
        #[arg(long)]
        name: Option<String>,
        /// Link it to an upstream skill it was copied from
        #[arg(long)]
        upstream: Option<String>,
        /// Upstream commit the copy started from (required with --upstream)
        #[arg(long)]
        base: Option<String>,
    },
    /// Scaffold a new skill
    New {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Lint workspace skills (or a skill directory)
    Lint {
        skills: Vec<String>,
        #[arg(long)]
        fix: bool,
        /// Frontmatter keys outside the Agent Skills spec are errors (like `skills-ref`)
        #[arg(long)]
        strict: bool,
    },
    /// Edit a skill (optionally on a branch); flips its deployment to dev mode
    Edit {
        skill: String,
        #[arg(long, short = 'b')]
        branch: Option<String>,
    },
    /// Commit a skill's changes and return its deployment to the active variant
    Commit {
        skill: String,
        #[arg(long, short = 'm')]
        message: String,
    },
    /// Choose which branch variant of a skill is deployed
    Use {
        /// name@branch (or name with --reset)
        spec: String,
        /// Machine-local override (tricks.work.toml)
        #[arg(long)]
        local: bool,
        #[arg(long)]
        reset: bool,
    },
    /// Show changes between versions of a workspace skill
    Diff {
        skill: String,
        /// base | upstream | head | working | candidate (the merge result, not yet applied)
        #[arg(long, default_value = "base")]
        from: String,
        #[arg(long, default_value = "working")]
        to: String,
    },
    /// Open a pull request with your change to the upstream skill
    Pr {
        skill: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Publish workspace skills to a distribution repository
    Publish {
        target: String,
        /// major | minor | patch | <version>
        #[arg(long)]
        bump: Option<String>,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        push: bool,
        #[arg(long)]
        pr: bool,
        #[arg(long)]
        accept_copyleft: bool,
    },
    /// Allow publishing a vendored skill whose licence would block it (requires a reason)
    AllowLicense { skill: String, justification: String },
    /// List registered workspaces
    Workspaces,
}

pub fn run(ctx: &Ctx, c: WsCmd) -> Result<()> {
    let json = ctx.opts.json;
    match c {
        WsCmd::Init { name, agent_skill } => {
            let r = workspace::init(ctx, name.as_deref(), agent_skill)?;
            emit(json, &r, |r| {
                println!("{} workspace `{}` at {}", if r.created { "created" } else { "registered" }, r.name, r.root);
                for p in &r.agent_skill {
                    println!("  agent skill → {p}");
                }
                if r.created {
                    println!("next: `tricks vendor owner/repo//skill` or `tricks new my-skill`");
                }
            });
        }
        WsCmd::Vendor { skill, name, path } => {
            let ws = workspace::require(ctx)?;
            let r = workspace::vendor(ctx, &ws, &skill, name.as_deref(), path.as_deref())?;
            emit(json, &r, print_vendor);
        }
        WsCmd::Import { folder, name, upstream, base } => {
            let ws = workspace::require(ctx)?;
            let r = workspace::import(ctx, &ws, &folder, name.as_deref(), upstream.as_deref(), base.as_deref())?;
            emit(json, &r, print_vendor);
        }
        WsCmd::New { name, description } => {
            let ws = workspace::require(ctx)?;
            let r = workspace::new_skill(&ws, &name, description.as_deref())?;
            emit(json, &r, |r| println!("created {} — edit {}/SKILL.md", r.name, r.path));
        }
        WsCmd::Lint { skills, fix, strict } => {
            let one_path = skills.len() == 1 && std::path::Path::new(&skills[0]).join("SKILL.md").is_file();
            let ws = workspace::current(ctx)?;
            let mut rep = match (&ws, one_path) {
                (Some(ws), false) => {
                    if fix {
                        let mut fixed = Vec::new();
                        for (n, s) in &ws.manifest.skills {
                            if skills.is_empty() || skills.contains(n) {
                                fixed.extend(crate::lint::fix_dir(&ws.root.join(&s.path))?.into_iter().map(|f| format!("{n}/{f}")));
                            }
                        }
                        let mut r = crate::lint::lint_workspace_opts(ctx, ws, &skills, strict)?;
                        r.fixed = fixed;
                        r
                    } else {
                        crate::lint::lint_workspace_opts(ctx, ws, &skills, strict)?
                    }
                }
                _ => {
                    let Some(p) = skills.first() else { bail!("not in a workspace: pass a skill directory") };
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
        WsCmd::Edit { skill, branch } => {
            let r = workspace::edit(ctx, &skill, branch.as_deref())?;
            emit(json, &r, |r| {
                if r.vendored {
                    println!("vendored {} into the workspace", r.name);
                }
                println!("{}", r.path);
                if let Some(b) = &r.branch {
                    eprintln!("editing `{}` on branch {b}; commit with `tricks commit {} -m \"…\"`", r.name, r.name);
                }
                for p in &r.placements {
                    eprintln!("  dev → {p}");
                }
            });
        }
        WsCmd::Commit { skill, message } => {
            let r = workspace::commit(ctx, &skill, &message)?;
            emit(json, &r, |r| {
                match &r.commit {
                    Some(c) => {
                        println!("committed {} {}{}", r.name, short(c), r.branch.as_deref().map(|b| format!(" on {b}")).unwrap_or_default())
                    }
                    None => println!("nothing to commit for {}", r.name),
                }
                if let Some(a) = &r.agent {
                    println!("  Tricks-Agent: {a}");
                }
            });
        }
        WsCmd::Use { spec, local, reset } => {
            let r = workspace::use_variant(ctx, &spec, local, reset)?;
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
        WsCmd::Diff { skill, from, to } => {
            let ws = workspace::require(ctx)?;
            let files = workspace::changed_files(ctx, &ws, &skill, &from, &to)?;
            let mut out = Vec::new();
            for f in &files {
                let a = workspace::version_file(ctx, &ws, &skill, &from, f)?.unwrap_or_default();
                let b = workspace::version_file(ctx, &ws, &skill, &to, f)?.unwrap_or_default();
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
        WsCmd::Pr { skill, title, body, dry_run } => {
            let r = crate::pr::pr(ctx, &skill, title.as_deref(), body.as_deref(), dry_run)?;
            emit(json, &r, |r| {
                println!("{} → {} (branch {})", r.skill, r.upstream, r.branch);
                println!("{}", r.diffstat);
                match &r.url {
                    Some(u) => println!("opened {u}"),
                    None => println!("dry run: prepared in {}", r.worktree),
                }
            });
        }
        WsCmd::Publish { target, bump, dry_run, push, pr, accept_copyleft } => {
            let r = crate::publish::publish(ctx, &crate::publish::PublishOptions { target, bump, dry_run, push, pr, accept_copyleft })?;
            let blocked = r.blocked;
            emit(json, &r, print_publish);
            if blocked {
                std::process::exit(1);
            }
        }
        WsCmd::AllowLicense { skill, justification } => {
            let ws = workspace::require(ctx)?;
            if justification.trim().len() < 10 {
                bail!("give a real justification (e.g. \"separate licence agreement with the vendor\")");
            }
            workspace::set_license_override(&ws, &skill, &justification)?;
            emit(json, &serde_json::json!({ "skill": skill, "justification": justification }), |_| {
                println!("{skill}: licence override recorded (shown in every publish pre-flight)")
            });
        }
        WsCmd::Workspaces => {
            let all = workspace::all_workspaces(ctx)?;
            let rows: Vec<serde_json::Value> =
                all.iter().map(|w| serde_json::json!({ "name": w.name, "root": w.root, "skills": w.manifest.skills.len() })).collect();
            emit(json, &rows, |rows| {
                if rows.is_empty() {
                    println!("no workspaces registered; run `tricks init` in a git repository");
                }
                for r in rows {
                    println!("{:<20} {:>3} skills  {}", r["name"].as_str().unwrap(), r["skills"], r["root"].as_str().unwrap());
                }
            });
        }
    }
    Ok(())
}

fn print_vendor(r: &workspace::VendorReport) {
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
    println!("  not committed yet: review and `git commit` when ready");
}

pub fn print_ws_install(r: &WsInstallReport) {
    println!("workspace {}: dev deployments", r.workspace);
    for (n, ps) in &r.skills {
        println!("  {n}");
        for p in ps {
            println!("    → {p}");
        }
    }
}

pub fn print_ws_update(r: &WsUpdateReport) {
    if r.items.is_empty() {
        println!("no vendored skills to update");
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

pub fn print_ws_outdated(r: &WsUpdateReport) {
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
        if let Some(m) = &i.message {
            println!("    {m}");
        }
    }
    if !any {
        println!("all vendored skills are up to date");
    }
}

pub fn print_ws_status(w: &WsStatus) {
    println!("workspace {} ({}{})", w.name, w.root, w.branch.as_deref().map(|b| format!(", branch {b}")).unwrap_or_default());
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
            notes.push(format!("update ready: {u}"));
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
