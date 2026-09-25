//! `tricks doctor`: environment, credentials and state checks.

use crate::ctx::Ctx;
use crate::github::TokenSource;
use anyhow::Result;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub version: String,
    pub checks: Vec<Check>,
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check { name: name.into(), ok, detail: detail.into() }
}

pub fn run(ctx: &Ctx) -> Result<DoctorReport> {
    let mut checks = Vec::new();
    match std::process::Command::new("git").arg("--version").output() {
        Ok(o) if o.status.success() => checks.push(check("git", true, String::from_utf8_lossy(&o.stdout).trim().to_string())),
        _ => checks.push(check("git", false, "git not found on PATH (required)")),
    }
    let gh = std::process::Command::new("gh").arg("--version").output().map(|o| o.status.success()).unwrap_or(false);
    checks.push(check("gh", gh, if gh { "GitHub CLI found" } else { "GitHub CLI not found (optional; used for tokens and PRs)" }));
    let src = ctx.gh.token_source("github.com");
    let (ok, detail) = match src {
        TokenSource::Env => (true, "token from TRICKS_GITHUB_TOKEN/GITHUB_TOKEN".to_string()),
        TokenSource::Gh => (true, "token from `gh auth token`".to_string()),
        TokenSource::VsCode => (true, "token from the VS Code GitHub session (in memory)".to_string()),
        TokenSource::None => {
            (false, "anonymous: public sources only, 60 API requests/hour, no GitHub code search. Run `gh auth login`.".to_string())
        }
    };
    checks.push(check("github.com credentials", ok, detail));
    if ok && !ctx.opts.offline {
        match ctx.gh.identity("github.com") {
            Ok((login, orgs)) => checks.push(check(
                "identity",
                true,
                format!("{login} (orgs: {})", if orgs.is_empty() { "none".into() } else { orgs.join(", ") }),
            )),
            Err(e) => checks.push(check("identity", false, format!("{e:#}"))),
        }
        #[derive(serde::Deserialize)]
        struct Rate {
            rate: RateInner,
        }
        #[derive(serde::Deserialize)]
        struct RateInner {
            remaining: i64,
            limit: i64,
        }
        if let Ok(r) = ctx.gh.api_json::<Rate>("github.com", "/rate_limit") {
            checks.push(check("API rate limit", r.rate.remaining > 50, format!("{}/{} remaining", r.rate.remaining, r.rate.limit)));
        }
    }
    let m = crate::user::config(ctx)?;
    for (name, path) in &m.source_repos {
        let p = ctx.paths.expand(path);
        let ok = p.join(crate::config::REPO_MANIFEST).is_file();
        checks.push(check(
            &format!("source repo {name}"),
            ok,
            if ok { ctx.paths.contract(&p) } else { format!("{} has no tricks.toml", p.display()) },
        ));
    }
    checks.push(check("config", true, ctx.paths.contract(&ctx.paths.config_dir)));
    checks.push(check("data", true, ctx.paths.contract(&ctx.paths.data_dir)));
    let store_entries = std::fs::read_dir(ctx.paths.store()).map(|r| r.count()).unwrap_or(0);
    checks.push(check("store", true, format!("{store_entries} revision(s)")));
    checks.push(check("index", true, format!("{} skill(s) indexed", crate::index::count(ctx)?)));
    for a in crate::agents::AGENTS.iter() {
        let d = a.user_path(&ctx.paths.home);
        let mode = if a.follows_links(&ctx.paths.store()) { "link" } else { "copy" };
        checks.push(check(
            &format!("agent {}", a.id),
            true,
            format!(
                "user {} ({}), project {}; {mode} mode; tested {}",
                ctx.paths.contract(&d),
                if d.exists() { "exists" } else { "not created yet" },
                a.project_dir,
                a.tested
            ),
        ));
    }
    // User-scope installs from New Tricks 0.2 and earlier keep working but are no
    // longer managed: install them with APM, `npx skills` or a plugin marketplace.
    let legacy = crate::links::legacy(ctx)?;
    if !legacy.is_empty() || !m.skills.is_empty() || ctx.paths.legacy_user_lock().exists() {
        checks.push(check(
            "user-scope installs",
            false,
            format!(
                "{} skill(s) installed by New Tricks 0.2 are still deployed but no longer managed; reinstall them with APM or                  `npx skills add`, then remove the old copies with `tricks unlink --legacy` (and the [skills] table / tricks.lock in {})",
                legacy.len().max(m.skills.len()),
                ctx.paths.contract(&ctx.paths.config_dir)
            ),
        ));
    }
    let unfinished = ctx.state.unfinished_ops()?;
    checks.push(check(
        "operations",
        unfinished.is_empty(),
        if unfinished.is_empty() {
            "no interrupted operations".to_string()
        } else {
            format!("{} interrupted operation(s); re-run them", unfinished.len())
        },
    ));
    let broken: Vec<String> = ctx
        .state
        .all_placements()?
        .into_iter()
        .filter(|p| crate::deploy::health(p) != "ok")
        .map(|p| format!("{} ({})", p.path, crate::deploy::health(&p)))
        .collect();
    checks.push(check("placements", broken.is_empty(), if broken.is_empty() { "all healthy".to_string() } else { broken.join(", ") }));
    Ok(DoctorReport { version: env!("CARGO_PKG_VERSION").into(), checks })
}

pub fn print(r: &DoctorReport) {
    println!("New Tricks {}", r.version);
    for c in &r.checks {
        println!("  {} {:<24} {}", if c.ok { "✓" } else { "✗" }, c.name, c.detail);
    }
}
