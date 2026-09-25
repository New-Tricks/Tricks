//! Command-line interface (spec §15). Every read command supports `--json`.

use crate::ctx::{CliUi, Ctx, Opts};
use crate::index::Filters;
use anyhow::Result;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
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
    /// Only skills that declare support for this agent
    #[arg(long)]
    pub agent: Option<String>,
    /// yours | org | official | starred | unknown
    #[arg(long)]
    pub trust: Option<String>,
    /// allow | weak-copyleft | strong-copyleft | non-commercial | block | unknown
    #[arg(long)]
    pub license: Option<String>,
    /// Only skills listed in this catalog (or from this repository)
    #[arg(long = "catalog", value_name = "CATALOG")]
    pub source: Option<String>,
    /// Only skills from repositories of this owner
    #[arg(long)]
    pub owner: Option<String>,
    /// Only skills in this catalog category
    #[arg(long)]
    pub category: Option<String>,
    /// Exclude skills that ship scripts
    #[arg(long)]
    pub no_scripts: bool,
    /// Maximum number of results
    #[arg(long, default_value_t = 20)]
    pub limit: usize,
    /// Re-index all catalogs first
    #[arg(long)]
    pub refresh: bool,
    /// Skip the live-query catalogs (skills.sh, Tessl, ClawHub, GitHub code search)
    #[arg(long)]
    pub no_live: bool,
}

#[derive(Subcommand)]
pub enum CatalogCmd {
    /// Add a repository, marketplace, well-known site or apm.yml/skills-lock.json
    Add {
        /// owner/repo, URL, path to apm.yml / skills-lock.json, or a site with /.well-known/agent-skills
        input: String,
        /// repo | marketplace | wellknown | pointers (detected when omitted)
        #[arg(long)]
        kind: Option<String>,
    },
    /// List catalogs
    List,
    /// Remove (or disable a default) catalog
    Remove {
        /// Catalog as shown by `tricks catalog list`
        input: String,
    },
    /// Re-index catalogs now
    Refresh {
        /// Only this catalog
        catalog: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum Cmd {
    // ------------------------------------------------------------ discover
    /// Search skills across all catalogs
    Search(SearchArgs),
    /// Show a skill: metadata, files, licence, risk, catalog signals, body
    #[command(visible_alias = "info")]
    Show {
        /// Skill id, URL, or name
        skill: String,
        /// Print one supporting file instead
        #[arg(long)]
        file: Option<String>,
    },
    /// Manage the catalogs search draws on
    #[command(subcommand)]
    Catalog(CatalogCmd),

    // ------------------------------------------------------------ start
    /// Make the current git repository a source repo
    Init {
        /// Source repo name (default: the directory name)
        #[arg(long)]
        name: Option<String>,
        /// Also install the bundled `new-tricks` agent skill into this repository
        #[arg(long)]
        agent_skill: bool,
    },
    /// Scaffold a new skill in the source repo
    New {
        /// Skill name (lowercase letters, digits and hyphens)
        name: String,
        /// Frontmatter description
        #[arg(long)]
        description: Option<String>,
    },
    /// Bring a skill into the source repo to customize it: an upstream skill or a local folder
    Vendor {
        /// Upstream skill (owner/repo//skill, URL, ClawHub or .well-known id) or a local folder
        skill: String,
        /// Name in the source repo (default: the skill's name)
        #[arg(long)]
        name: Option<String>,
        /// Path in the source repo (default: skills/<name>)
        #[arg(long)]
        path: Option<String>,
        /// For a local folder: the upstream skill it was copied from
        #[arg(long)]
        upstream: Option<String>,
        /// For a local folder: the upstream commit the copy started from (with --upstream)
        #[arg(long)]
        base: Option<String>,
    },

    // ------------------------------------------------------------ work
    /// Edit a skill (optionally on a branch); its links follow the edits live
    Edit {
        /// Source repo skill (an upstream skill is vendored first)
        skill: String,
        /// Edit on this branch, in its own worktree (created if needed)
        #[arg(long, short = 'b', conflicts_with = "done")]
        branch: Option<String>,
        /// Stop editing: links return to the skill's active variant (commit with git)
        #[arg(long)]
        done: bool,
    },
    /// Choose which branch variant of a skill its links deploy
    Use {
        /// <skill>@<branch>, or <skill> with --reset
        #[arg(value_name = "SKILL@BRANCH")]
        spec: String,
        /// Machine-local choice (tricks.work.toml, not committed)
        #[arg(long)]
        local: bool,
        /// Back to the main checkout
        #[arg(long)]
        reset: bool,
    },
    /// Merge upstream changes into vendored skills (left uncommitted for review)
    Merge {
        /// Only this skill (also merges a pinned skill)
        skill: Option<String>,
        /// Show what would be merged, with a risk summary; change nothing
        #[arg(long, conflicts_with_all = ["cont", "abort"])]
        dry_run: bool,
        /// Finish a merge after resolving its conflicts
        #[arg(long = "continue", conflicts_with = "abort")]
        cont: bool,
        /// Abandon a merge and restore your version
        #[arg(long)]
        abort: bool,
    },
    /// Show changes between versions of a source repo skill
    Diff {
        /// Source repo skill
        skill: String,
        /// base | upstream | head | working | candidate (the merge result, not yet applied)
        #[arg(long, default_value = "base")]
        from: String,
        /// Same choices as --from
        #[arg(long, default_value = "working")]
        to: String,
    },
    /// Source repo skills, variants, upstream changes and active links
    #[command(visible_alias = "ls")]
    Status,

    // ------------------------------------------------------------ validate
    /// Link skills into agent directories to test them (all source repo skills when none is named)
    Link {
        /// Source repo skill (or skill@branch), local folder, or an upstream skill to try without vendoring
        skill: Option<String>,
        /// Link into this project (default for upstream skills: the current directory)
        #[arg(long)]
        to: Option<String>,
        /// Link into the user-level agent directories (default for source repo skills)
        #[arg(long)]
        global: bool,
        /// Agents to link for (default: the source repo's, else the user setting)
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
        /// Copy instead of symlinking
        #[arg(long)]
        copy: bool,
        /// Temporarily replace an existing skill of the same name (restored on unlink)
        #[arg(long)]
        shadow: bool,
    },
    /// Remove links (all of the source repo's when none is named)
    Unlink {
        /// Skill to unlink
        skill: Option<String>,
        /// Only links in this project
        #[arg(long)]
        to: Option<String>,
        /// Only links in the user-level agent directories
        #[arg(long)]
        global: bool,
        /// Every link, everywhere
        #[arg(long)]
        all: bool,
        /// Remove skills installed at user scope by New Tricks 0.2 and earlier
        #[arg(long)]
        legacy: bool,
    },
    /// Lint source repo skills (or a skill directory)
    Lint {
        /// Skills (default: all) or one skill directory
        skills: Vec<String>,
        /// Apply safe fixes
        #[arg(long)]
        fix: bool,
        /// Frontmatter keys outside the Agent Skills spec are errors (like `skills-ref`)
        #[arg(long)]
        strict: bool,
    },

    // ------------------------------------------------------------ ship
    /// Publish source repo skills to a distribution repository
    Publish {
        /// Publish target from tricks.toml
        target: String,
        /// major | minor | patch | <version>
        #[arg(long)]
        bump: Option<String>,
        /// Run the gates and show the changes; write nothing
        #[arg(long)]
        dry_run: bool,
        /// Push the published commit (and tag) to the target's remote
        #[arg(long, conflicts_with = "pr")]
        push: bool,
        /// Open a pull request on the target instead of pushing to its default branch
        #[arg(long)]
        pr: bool,
        /// Allow strong-copyleft skills in a public target
        #[arg(long)]
        accept_copyleft: bool,
    },
    /// Offer your change to a vendored skill back upstream as a pull request
    Contribute {
        /// Vendored skill
        skill: String,
        /// Pull request title
        #[arg(long)]
        title: Option<String>,
        /// Pull request body
        #[arg(long)]
        body: Option<String>,
        /// Prepare the branch locally; do not fork, push or open the pull request
        #[arg(long)]
        dry_run: bool,
    },

    // ------------------------------------------------------------ maintain
    /// Check environment, credentials, agents and state
    Doctor,
    /// Update tricks itself
    SelfUpdate {
        /// Only check for a newer release
        #[arg(long)]
        check: bool,
    },

    // ------------------------------------------------------------ plumbing
    /// JSON-RPC server for the VS Code extension
    #[command(hide = true)]
    Serve {
        /// Speak JSON-RPC over stdin/stdout
        #[arg(long)]
        stdio: bool,
    },
    /// One-line status for agent status lines (cached state only, never the network)
    #[command(hide = true)]
    Statusline,
    /// Prune store entries no link or merge needs (runs automatically after unlink)
    #[command(hide = true)]
    Gc {
        /// Show what would be removed
        #[arg(long)]
        dry_run: bool,
    },
}

/// Top-level help, grouped by what you are doing.
const GROUPS: &[(&str, &[&str])] = &[
    ("Discover", &["search", "show", "catalog"]),
    ("Start (in a source repo)", &["init", "new", "vendor"]),
    ("Work on skills", &["edit", "use", "merge", "diff", "status"]),
    ("Validate", &["link", "unlink", "lint"]),
    ("Ship", &["publish", "contribute"]),
    ("Maintain", &["doctor", "self-update"]),
];

pub fn command() -> clap::Command {
    let cmd = Cli::command();
    let mut t = String::from("{about-with-newline}\n{usage-heading} {usage}\n");
    for (title, names) in GROUPS {
        t.push_str(&format!("\n{title}:\n"));
        for n in *names {
            let sc = cmd.find_subcommand(n).unwrap_or_else(|| panic!("no subcommand {n}"));
            let aliases: Vec<&str> = sc.get_visible_aliases().collect();
            let label = if aliases.is_empty() { n.to_string() } else { format!("{n} ({})", aliases.join(", ")) };
            t.push_str(&format!("  {label:<14} {}\n", sc.get_about().map(|a| a.to_string()).unwrap_or_default()));
        }
    }
    t.push_str("\nOptions:\n{options}{after-help}");
    cmd.help_template(t)
}

pub fn main() -> std::process::ExitCode {
    let cli = match command().try_get_matches().and_then(|m| Cli::from_arg_matches(&m)) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
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
    if !matches!(cli.cmd, Cmd::Statusline) {
        crate::source_repo::reconcile(&ctx)?;
    }
    let json = cli.json;
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
        Cmd::Catalog(sc) => run_catalog(&ctx, sc)?,
        Cmd::Status => {
            let r = crate::cli_repo::status(&ctx)?;
            emit(json, &r, crate::cli_repo::print_status);
        }
        Cmd::Link { skill, to, global, agents, copy, shadow } => {
            let o = crate::links::LinkOptions { to: to.as_deref(), global, agents: &agents, copy, shadow };
            let r = crate::links::link(&ctx, skill.as_deref(), &o)?;
            emit(json, &r, |r| {
                for l in &r.links {
                    println!("linked {}{} into {}", l.name, if l.trial { " (trial)" } else { "" }, l.scope);
                    for (a, p, m) in &l.placements {
                        println!("  {a:<8} {p} ({m})");
                    }
                }
                for (n, e) in &r.errors {
                    println!("failed {n}: {e}");
                }
            });
        }
        Cmd::Unlink { skill, to, global, all, legacy } => {
            let o = crate::links::UnlinkOptions { to: to.as_deref(), global, all, legacy };
            let r = crate::links::unlink(&ctx, skill.as_deref(), &o)?;
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
        other => crate::cli_repo::run(&ctx, other)?,
    }
    Ok(())
}

fn run_catalog(ctx: &Ctx, sc: CatalogCmd) -> Result<()> {
    let json = ctx.opts.json;
    match sc {
        CatalogCmd::Add { input, kind } => {
            let (key, k) = crate::catalogs::add(ctx, &input, kind.as_deref())?;
            let rep = crate::catalogs::refresh(ctx, true, Some(&key))?;
            emit(json, &serde_json::json!({ "catalog": key, "kind": k, "indexed": rep.indexed, "errors": rep.errors }), |_| {
                println!("added {key} ({})", k.as_str());
                for (s, n) in &rep.indexed {
                    println!("  indexed {n} skill(s) from {s}");
                }
            });
        }
        CatalogCmd::List => {
            let l = crate::catalogs::list(ctx)?;
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
                let live = crate::user::config(ctx).map(|m| m.settings.live).unwrap_or_default();
                let gh = if ctx.gh.token("github.com").is_some() { "" } else { " (GitHub code search needs `gh auth login`)" };
                println!("live: {}{gh}", live.join(", "));
            });
        }
        CatalogCmd::Remove { input } => {
            let k = crate::catalogs::remove(ctx, &input)?;
            emit(json, &serde_json::json!({ "removed": k }), |_| println!("removed {k}"));
        }
        CatalogCmd::Refresh { catalog } => {
            let rep = crate::catalogs::refresh(ctx, true, catalog.as_deref())?;
            emit(json, &rep, |r| {
                for (s, n) in &r.indexed {
                    println!("indexed {n:>4} skill(s) from {s}");
                }
                for (s, e) in &r.errors {
                    println!("failed  {s}: {e}");
                }
            });
        }
    }
    Ok(())
}

pub fn do_search(ctx: &Ctx, a: &SearchArgs) -> Result<Vec<crate::index::SearchResult>> {
    let q = a.query.join(" ");
    let _ = crate::catalogs::refresh(ctx, a.refresh, None)?;
    if !a.no_live {
        crate::live::search(ctx, &q, &crate::user::config(ctx)?.settings.live);
    }
    let f = Filters {
        agent: a.agent.clone(),
        trust: a.trust.clone(),
        license: a.license.clone(),
        source: a.source.clone(),
        owner: a.owner.clone(),
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
        if r.linked {
            tags.push("linked".into());
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
    let state: Vec<&str> =
        [(r.vendored, "vendored"), (r.linked, "linked for a trial")].iter().filter(|(on, _)| *on).map(|(_, s)| *s).collect();
    if !state.is_empty() {
        println!("  state    {}", state.join(", "));
    }
    println!("  files:");
    for f in &r.files {
        println!("    {:<50} {:>7}{}", f.path, f.size, if f.script { "  script" } else { "" });
    }
    if let Some(e) = &r.frontmatter_error {
        println!("  frontmatter error: {e}");
    }
    println!("\n{}", r.body.trim_end());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_groups_cover_every_visible_command() {
        let cmd = Cli::command();
        let grouped: Vec<&str> = GROUPS.iter().flat_map(|(_, n)| n.iter().copied()).collect();
        for sc in cmd.get_subcommands().filter(|s| !s.is_hide_set() && s.get_name() != "help") {
            assert!(grouped.contains(&sc.get_name()), "`{}` is missing from the grouped help", sc.get_name());
        }
        command().debug_assert();
    }
}
