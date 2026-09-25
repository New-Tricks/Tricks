//! Command-line interface (spec §15). Every read command supports `--json`.

use crate::config::Policy;
use crate::ctx::{CliUi, Ctx, Opts};
use crate::index::Filters;
use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

#[derive(Parser)]
#[command(
    name = "tricks",
    version,
    about = "New Tricks: teach your agents new tricks. The design-time workbench for agent skills.",
    propagate_version = true
)]
pub struct Cli {
    /// Machine-readable output
    #[arg(long, global = true)]
    pub json: bool,
    /// Never touch the network
    #[arg(long, global = true)]
    pub offline: bool,
    /// Answer yes to confirmations
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,
    /// Less progress output
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Args, Debug, Default, Clone)]
pub struct SearchArgs {
    /// Free-text query (empty lists everything)
    pub query: Vec<String>,
    #[arg(long)]
    pub agent: Option<String>,
    /// yours | org | official | starred | unknown
    #[arg(long)]
    pub trust: Option<String>,
    /// allow | weak-copyleft | strong-copyleft | non-commercial | block | unknown
    #[arg(long)]
    pub license: Option<String>,
    /// Only skills listed in this catalog (or from this repository)
    #[arg(long = "catalog")]
    pub source: Option<String>,
    #[arg(long)]
    pub owner: Option<String>,
    #[arg(long)]
    pub category: Option<String>,
    /// Only skills installed at user scope
    #[arg(long)]
    pub installed: bool,
    /// Exclude skills that ship scripts
    #[arg(long)]
    pub no_scripts: bool,
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Re-index all catalogs first
    #[arg(long)]
    pub refresh: bool,
    /// Skip live-query adapters (skills.sh, GitHub code search)
    #[arg(long)]
    pub no_live: bool,
}

#[derive(Subcommand)]
pub enum CatalogCmd {
    /// Add a repository, marketplace, well-known site or apm.yml/skills-lock.json
    Add {
        input: String,
        /// repo | marketplace | wellknown | pointers
        #[arg(long)]
        kind: Option<String>,
    },
    /// List catalogs
    List,
    /// Remove (or disable a default) catalog
    Remove { input: String },
    /// Re-index catalogs now
    Refresh { catalog: Option<String> },
}

#[derive(Subcommand)]
pub enum Cmd {
    /// Search skills across all catalogs
    Search(SearchArgs),
    /// Show a skill: metadata, files, licence, risk, body
    Show {
        skill: String,
        /// Print one supporting file instead
        #[arg(long)]
        file: Option<String>,
    },
    /// Manage the catalogs search draws on
    #[command(subcommand)]
    Catalog(CatalogCmd),
    /// Install a skill at user scope for agents
    Add {
        skill: String,
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
        /// review | auto | unsafe-auto | pinned | paused
        #[arg(long)]
        update: Option<String>,
        #[arg(long)]
        copy: bool,
        #[arg(long)]
        shadow: bool,
    },
    /// Remove a user skill and its placements
    Remove { skill: String },
    /// Sync installs to the manifest (in a source repo: link its skills in dev mode)
    Install {
        #[arg(long)]
        frozen: bool,
        /// Operate at user scope even inside a source repo
        #[arg(long, short = 'g')]
        global: bool,
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
    },
    /// Apply updates (user scope), or merge upstream changes (source repo)
    Update {
        skill: Option<String>,
        #[arg(long, short = 'g')]
        global: bool,
        #[arg(long = "continue")]
        cont: bool,
        #[arg(long)]
        abort: bool,
    },
    /// List available updates
    Outdated {
        #[arg(long, short = 'g')]
        global: bool,
    },
    /// Installed skills, placements, test links and pending work
    Status,
    /// Roll a user skill back to its previous revision (and pin it)
    Rollback { skill: String },
    /// Set the update policy of a user skill
    Policy { skill: String, policy: String },
    /// Deploy a skill into a project or globally for testing
    Link {
        skill: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        global: bool,
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
        #[arg(long)]
        copy: bool,
        #[arg(long)]
        shadow: bool,
    },
    /// Remove test deployments
    Unlink {
        skill: Option<String>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        all: bool,
    },
    /// List agent integrations and their directories
    Agents,
    /// One-line status for agent status lines (cached state only, never the network)
    Statusline,
    /// Install (or remove) the bundled `new-tricks` agent skill
    AgentSkill {
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
        #[arg(long)]
        remove: bool,
    },
    /// Prune unreferenced store entries
    Gc {
        #[arg(long)]
        dry_run: bool,
    },
    /// Check environment, credentials and state
    Doctor,
    /// JSON-RPC server for the VS Code extension
    Serve {
        #[arg(long)]
        stdio: bool,
    },
    /// Update tricks itself
    SelfUpdate {
        #[arg(long)]
        check: bool,
    },
    #[command(flatten)]
    SourceRepo(crate::cli_repo::RepoCmd),
}

pub fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            if json {
                let kind = if e.downcast_ref::<crate::ctx::ConfirmationRequired>().is_some() { "confirmation_required" } else { "error" };
                println!("{}", serde_json::json!({ "error": format!("{e:#}"), "kind": kind }));
            } else {
                eprintln!("error: {e:#}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

pub fn emit<T: Serialize>(json: bool, v: &T, human: impl FnOnce(&T)) {
    if json {
        println!("{}", serde_json::to_string_pretty(v).unwrap());
    } else {
        human(v);
    }
}

fn run(cli: Cli) -> Result<()> {
    if let Cmd::Serve { .. } = cli.cmd {
        return crate::rpc::serve();
    }
    let offline = cli.offline || matches!(cli.cmd, Cmd::Statusline);
    let opts = Opts { offline, yes: cli.yes, json: cli.json, cwd: std::env::current_dir()? };
    let ctx = Ctx::new(opts, Box::new(CliUi { yes: cli.yes, quiet: cli.quiet || cli.json }))?;
    let json = cli.json;
    first_run_offer(&ctx, &cli.cmd, cli.yes);
    match cli.cmd {
        Cmd::Search(a) => {
            let res = do_search(&ctx, &a)?;
            emit(json, &res, print_search);
        }
        Cmd::Show { skill, file } => match file {
            Some(f) => {
                let (_, bytes) = crate::inspect::read_file(&ctx, &skill, &f)?;
                if json {
                    println!("{}", serde_json::json!({ "path": f, "content": String::from_utf8_lossy(&bytes) }));
                } else {
                    print!("{}", String::from_utf8_lossy(&bytes));
                }
            }
            None => {
                let r = crate::inspect::show(&ctx, &skill)?;
                emit(json, &r, print_show);
            }
        },
        Cmd::Catalog(sc) => match sc {
            CatalogCmd::Add { input, kind } => {
                let (key, k) = crate::catalogs::add(&ctx, &input, kind.as_deref())?;
                let rep = crate::catalogs::refresh(&ctx, true, Some(&key))?;
                emit(json, &serde_json::json!({ "catalog": key, "kind": k, "indexed": rep.indexed, "errors": rep.errors }), |_| {
                    println!("added {key} ({})", k.as_str());
                    for (s, n) in &rep.indexed {
                        println!("  indexed {n} skill(s) from {s}");
                    }
                });
            }
            CatalogCmd::List => {
                let l = crate::catalogs::list(&ctx)?;
                emit(json, &l, |l| {
                    for s in l {
                        let when = s.last_indexed.map(ago).unwrap_or_else(|| "never".into());
                        println!(
                            "{:<60} {:<12} {:>5} skills  indexed {}{}{}",
                            s.key,
                            s.kind.as_str(),
                            s.skills,
                            when,
                            if s.default { "  (default)" } else { "" },
                            if s.disabled { "  [disabled]" } else { "" }
                        );
                    }
                    let live = crate::user::load_manifest(&ctx).map(|m| m.settings.live).unwrap_or_default();
                    let gh = if ctx.gh.token("github.com").is_some() { "" } else { " (GitHub code search needs `gh auth login`)" };
                    println!("live: {}{gh}", live.join(", "));
                });
            }
            CatalogCmd::Remove { input } => {
                let k = crate::catalogs::remove(&ctx, &input)?;
                emit(json, &serde_json::json!({ "removed": k }), |_| println!("removed {k}"));
            }
            CatalogCmd::Refresh { catalog } => {
                let rep = crate::catalogs::refresh(&ctx, true, catalog.as_deref())?;
                emit(json, &rep, |r| {
                    for (s, n) in &r.indexed {
                        println!("indexed {n:>4} skill(s) from {s}");
                    }
                    for (s, e) in &r.errors {
                        println!("failed  {s}: {e}");
                    }
                });
            }
        },
        Cmd::Add { skill, agents, update, copy, shadow } => {
            let policy = update.as_deref().map(Policy::parse).transpose()?;
            let r = crate::user::add(&ctx, &skill, &agents, policy, copy, shadow)?;
            emit(json, &r, |r| {
                println!("installed {} ({})", r.name, r.canonical);
                for p in &r.placements {
                    println!("  → {p}");
                }
                if let Some(l) = &r.license_class {
                    println!("  licence: {l}");
                }
                if !r.risk.is_empty() {
                    println!("  risk: {}", r.risk.join("; "));
                }
            });
        }
        Cmd::Remove { skill } => {
            let r = crate::user::remove(&ctx, &skill)?;
            emit(json, &r, |r| {
                println!("removed {skill}");
                for p in r {
                    println!("  - {p}");
                }
            });
        }
        Cmd::Install { frozen, global, agents } => {
            if !global && let Some(ws) = crate::source_repo::current(&ctx)? {
                let r = crate::source_repo::install_dev(&ctx, &ws, &agents)?;
                emit(json, &r, crate::cli_repo::print_repo_install);
                return Ok(());
            }
            let r = crate::user::install(&ctx, frozen)?;
            emit(json, &r, |r| {
                for s in &r.skills {
                    let d = s.detail.as_deref().map(|d| format!("  {d}")).unwrap_or_default();
                    println!("{:<14} {}{}", s.action, if s.name.is_empty() { &s.id } else { &s.name }, d);
                }
                if r.skills.is_empty() {
                    println!("no skills in the user config; add one with `tricks add owner/repo//skill`");
                }
                if r.updates_ready > 0 {
                    println!("{} update(s) ready — run `tricks update`", r.updates_ready);
                }
            });
        }
        Cmd::Update { skill, global, cont, abort } => {
            if !global && let Some(ws) = crate::source_repo::current(&ctx)? {
                let r = crate::source_repo::update(&ctx, &ws, skill.as_deref(), cont, abort)?;
                emit(json, &r, crate::cli_repo::print_repo_update);
                return Ok(());
            }
            let r = crate::user::update(&ctx, skill.as_deref())?;
            emit(json, &r, |r| {
                if r.is_empty() {
                    println!("everything is up to date");
                }
                for u in r {
                    println!("{} {}: {} → {}", if u.applied { "updated" } else { "skipped" }, u.name, u.from_ref, u.to_ref);
                    for c in &u.changes {
                        println!("    {c}");
                    }
                }
            });
        }
        Cmd::Outdated { global } => {
            if !global && let Some(ws) = crate::source_repo::current(&ctx)? {
                let r = crate::source_repo::outdated(&ctx, &ws)?;
                emit(json, &r, crate::cli_repo::print_repo_outdated);
                return Ok(());
            }
            let r = crate::user::outdated(&ctx)?;
            emit(json, &r, |r| {
                if r.is_empty() {
                    println!("everything is up to date");
                }
                for p in r {
                    println!("{:<24} {} → {}  ({})", p.name, p.from_ref, p.to_ref, p.policy);
                    for d in &p.details {
                        println!("    {d}");
                    }
                }
            });
        }
        Cmd::Status => {
            let r = crate::user::status(&ctx)?;
            let ws = crate::source_repo::current(&ctx)?.map(|w| crate::source_repo::status(&ctx, &w)).transpose()?;
            emit(json, &serde_json::json!({ "user": r, "source_repo": ws }), |_| print_status(&r, ws.as_ref()));
        }
        Cmd::Rollback { skill } => {
            let r = crate::user::rollback(&ctx, &skill)?;
            emit(json, &r, |r| {
                println!(
                    "rolled back {} {} → {} (pinned; `tricks policy {skill} review` to resume updates)",
                    r.id,
                    short(&r.from),
                    short(&r.to)
                )
            });
        }
        Cmd::Policy { skill, policy } => {
            let p = Policy::parse(&policy)?;
            let id = crate::user::set_policy(&ctx, &skill, p)?;
            emit(json, &serde_json::json!({ "id": id, "update": p.as_str() }), |_| println!("{id}: update = {}", p.as_str()));
        }
        Cmd::Link { skill, to, global, agents, copy, shadow } => {
            let r = crate::links::link(&ctx, &skill, to.as_deref(), global, &agents, copy, shadow)?;
            emit(json, &r, |r| {
                println!("linked {} into {}", r.name, r.scope);
                for (a, p, m) in &r.placements {
                    println!("  {a:<8} {p} ({m})");
                }
            });
        }
        Cmd::Unlink { skill, to, global, all } => {
            let r = crate::links::unlink(&ctx, skill.as_deref(), to.as_deref(), global, all)?;
            emit(json, &r, |r| {
                for p in &r.removed {
                    println!("removed {p}");
                }
                if r.removed.is_empty() {
                    println!("nothing to unlink");
                }
            });
        }
        Cmd::Statusline => {
            let line = crate::statusline::line(&ctx)?;
            if json {
                println!("{}", serde_json::json!({ "line": line }));
            } else if !line.is_empty() {
                println!("{line}");
            }
        }
        Cmd::AgentSkill { agents, remove } => {
            let paths = if remove { crate::agentskill::remove(&ctx)? } else { crate::agentskill::install(&ctx, &agents)? };
            emit(json, &serde_json::json!({ "removed": remove, "paths": paths }), |_| {
                for p in &paths {
                    println!("{} {p}", if remove { "removed" } else { "installed" });
                }
                if paths.is_empty() {
                    println!("nothing to do");
                }
            });
        }
        Cmd::Agents => {
            let rows: Vec<serde_json::Value> = crate::agents::AGENTS
                .iter()
                .map(|a| {
                    let target = ctx.paths.store();
                    serde_json::json!({
                        "id": a.id, "name": a.display,
                        "user_dir": ctx.paths.contract(&a.user_path(&ctx.paths.home)),
                        "project_dir": a.project_dir,
                        "mode": if a.follows_links(&target) { "link" } else { "copy" },
                        "tested": a.tested,
                    })
                })
                .collect();
            emit(json, &rows, |rows| {
                for r in rows {
                    println!(
                        "{:<8} {:<15} user {:<20} project {:<16} {} (tested {})",
                        r["id"].as_str().unwrap(),
                        r["name"].as_str().unwrap(),
                        r["user_dir"].as_str().unwrap(),
                        r["project_dir"].as_str().unwrap(),
                        r["mode"].as_str().unwrap(),
                        r["tested"].as_str().unwrap()
                    );
                }
            });
        }
        Cmd::Gc { dry_run } => {
            let r = crate::store::gc(&ctx, dry_run)?;
            emit(json, &r, |r| {
                println!("{} {} store entr(ies); kept {}", if dry_run { "would remove" } else { "removed" }, r.removed.len(), r.kept)
            });
        }
        Cmd::Doctor => {
            let r = crate::doctor::run(&ctx)?;
            emit(json, &r, crate::doctor::print);
        }
        Cmd::SelfUpdate { check } => {
            let r = crate::selfupdate::run(&ctx, check)?;
            emit(json, &r, |r| println!("{}", r.message));
        }
        Cmd::Serve { .. } => unreachable!(),
        Cmd::SourceRepo(w) => crate::cli_repo::run(&ctx, w)?,
    }
    Ok(())
}

/// Offer the bundled agent skill once, on the first interactive run (spec §12).
fn first_run_offer(ctx: &Ctx, cmd: &Cmd, yes: bool) {
    let skip = yes
        || ctx.opts.json
        || !ctx.ui.interactive()
        || matches!(cmd, Cmd::Serve { .. } | Cmd::Statusline | Cmd::AgentSkill { .. } | Cmd::SelfUpdate { .. } | Cmd::Doctor);
    if skip || !crate::agentskill::should_offer(ctx).unwrap_or(false) {
        return;
    }
    let details = vec![
        "It teaches Claude Code, Codex, Copilot and Cursor to search, preview, lint and draft skill changes on branches.".to_string(),
        "It pre-approves only read-only and branch-confined `tricks` commands; installs and publishes still ask you.".to_string(),
        "Change your mind any time: `tricks agent-skill` / `tricks agent-skill --remove`.".to_string(),
    ];
    let _ = crate::agentskill::mark_offered(ctx);
    match ctx.ui.confirm("Install the New Tricks agent skill for your default agents?", &details) {
        Ok(true) => match crate::agentskill::install(ctx, &[]) {
            Ok(paths) => paths.iter().for_each(|p| ctx.ui.info(&format!("installed agent skill → {p}"))),
            Err(e) => ctx.ui.warn(&format!("could not install the agent skill: {e:#}")),
        },
        _ => ctx.ui.info("skipped; run `tricks agent-skill` later if you want it"),
    }
}

pub fn do_search(ctx: &Ctx, a: &SearchArgs) -> Result<Vec<crate::index::SearchResult>> {
    let q = a.query.join(" ");
    let _ = crate::catalogs::refresh(ctx, a.refresh, None)?;
    if !a.no_live && !ctx.opts.offline && !q.trim().is_empty() {
        let live = crate::user::load_manifest(ctx)?.settings.live;
        let on = |c: &str| live.iter().any(|x| x == c);
        let report = |name: &str, r: Result<usize>| {
            if let Err(e) = r {
                ctx.ui.warn(&format!("{name}: {e:#}"));
            }
        };
        if on("skills.sh") {
            report("skills.sh", crate::catalogs::live_skills_sh(ctx, &q, 6));
        }
        if on("tessl") {
            report("Tessl", crate::tessl::live_search(ctx, &q, 20));
        }
        if on("clawhub") {
            report("ClawHub", crate::clawhub::live_search(ctx, &q, 20));
        }
        if on("github") {
            report("GitHub code search", crate::catalogs::live_github_search(ctx, &q, 5));
        }
    }
    let f = Filters {
        agent: a.agent.clone(),
        trust: a.trust.clone(),
        license: a.license.clone(),
        source: a.source.clone(),
        owner: a.owner.clone(),
        installed: a.installed,
        no_scripts: a.no_scripts,
        category: a.category.clone(),
    };
    crate::index::search(ctx, &q, &f, a.limit)
}

pub fn short(c: &str) -> &str {
    crate::id::short_commit(c)
}

pub fn ago(t: i64) -> String {
    let d = crate::state::now() - t;
    match d {
        d if d < 120 => "just now".into(),
        d if d < 7200 => format!("{}m ago", d / 60),
        d if d < 172_800 => format!("{}h ago", d / 3600),
        d => format!("{}d ago", d / 86_400),
    }
}

fn short_id(id: &str) -> String {
    id.strip_prefix("github.com/").unwrap_or(id).to_string()
}

fn print_search(res: &Vec<crate::index::SearchResult>) {
    if res.is_empty() {
        println!("no results");
        return;
    }
    for r in res {
        let mut tags = vec![r.trust.clone(), format!("licence:{}", r.license_class)];
        if let Some(i) = r.installs {
            tags.push(format!("{} installs", human_n(i)));
        }
        if let Some(s) = r.stars {
            tags.push(format!("★{}", human_n(s)));
        }
        if let Some(q) = r.signals.get("tessl").and_then(|t| t.get("quality")).and_then(|x| x.as_f64()) {
            tags.push(format!("Tessl quality {:.0}%", q * 100.0));
        }
        if r.installed {
            tags.push("installed".into());
        }
        if r.vendored {
            tags.push("vendored".into());
        }
        println!("{}  {}", r.name, short_id(&r.id));
        let d: String = r.description.chars().take(160).collect();
        if !d.is_empty() {
            println!("    {d}");
        }
        let mut extra = Vec::new();
        if !r.listed_in.is_empty() {
            extra.push(format!("in {} catalog(s)", r.listed_in.len()));
        }
        if !r.duplicates.is_empty() {
            extra.push(format!("{} identical cop(ies)", r.duplicates.len()));
        }
        if r.variants > 0 {
            extra.push(format!("{} variant(s)", r.variants));
        }
        if !r.risk.is_empty() {
            extra.push(r.risk.join(", "));
        }
        println!("    [{}]{}", tags.join(" · "), if extra.is_empty() { String::new() } else { format!("  {}", extra.join(" · ")) });
    }
}

pub fn human_n(n: i64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.1}M", n as f64 / 1e6),
        n if n >= 1_000 => format!("{:.1}k", n as f64 / 1e3),
        n => n.to_string(),
    }
}

fn print_show(r: &crate::inspect::ShowReport) {
    println!("{}  {}", r.name, r.canonical);
    if let Some(d) = &r.description {
        println!("  {d}");
    }
    println!("  commit   {} ({} {})", short(&r.commit), r.ref_kind, r.ref_name);
    println!("  trust    {}", r.trust);
    println!(
        "  licence  {} [{}] via {}{}",
        r.license.spdx.as_deref().unwrap_or("none"),
        r.license.class,
        r.license.source,
        if r.license.class != "block" && r.license.confidence > 0.0 && r.license.confidence < 1.0 {
            format!(" ({:.0}% match)", r.license.confidence * 100.0)
        } else {
            String::new()
        }
    );
    if !r.risk_summary.is_empty() {
        println!("  risk     {}", r.risk_summary.join("; "));
    }
    if let Some(i) = r.installs {
        println!("  installs {}", human_n(i));
    }
    if !r.listed_in.is_empty() {
        println!("  listed   {}", r.listed_in.join(", "));
    }
    for (catalog, sig) in &r.signals {
        let parts: Vec<String> = sig
            .as_object()
            .map(|o| {
                o.iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| format!("{k}={}", v.as_str().map(String::from).unwrap_or_else(|| v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        println!("  {catalog:<8} {}", parts.join(" "));
    }
    println!("  state    {}{}", if r.installed { "installed" } else { "not installed" }, if r.vendored { ", vendored" } else { "" });
    println!("  files:");
    for f in &r.files {
        println!("    {:<50} {:>7}{}", f.path, f.size, if f.script { "  script" } else { "" });
    }
    if let Some(e) = &r.frontmatter_error {
        println!("  frontmatter error: {e}");
    }
    println!("\n{}", r.body.trim_end());
}

fn print_status(r: &crate::user::StatusReport, ws: Option<&crate::source_repo::RepoStatus>) {
    if r.skills.is_empty() {
        println!("user scope: no skills installed");
    } else {
        println!("user skills:");
    }
    for s in &r.skills {
        let mut notes = vec![s.policy.clone()];
        if s.ahead_of_lock {
            notes.push(format!("deployed {} (ahead of lock)", s.deployed_commit.as_deref().map(short).unwrap_or("")));
        }
        if let Some(p) = &s.pending {
            notes.push(format!("update ready: {p}"));
        }
        println!("  {:<24} {} {} [{}]", s.name, s.ref_name, short(&s.commit), notes.join(", "));
        for p in &s.placements {
            let h = if p.health == "ok" { String::new() } else { format!("  !! {}", p.health) };
            println!("      {:<8} {} ({}){}", p.agent, p.path, p.mode, h);
        }
    }
    if !r.links.is_empty() {
        println!("test deployments:");
        for p in &r.links {
            let h = if p.health == "ok" { String::new() } else { format!("  !! {}", p.health) };
            println!("  {:<8} {} → {} ({}, {}){}", p.agent, p.path, short_id(&p.skill), p.mode, p.origin, h);
        }
    }
    if r.updates_ready > 0 {
        println!("{} update(s) ready — run `tricks update`", r.updates_ready);
    }
    for u in &r.unfinished_operations {
        println!("interrupted operation: {u} (re-run the command to recover)");
    }
    if let Some(w) = ws {
        crate::cli_repo::print_repo_status(w);
    }
}
