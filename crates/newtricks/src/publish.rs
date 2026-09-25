//! `tricks publish` (spec §11): source repo → distribution repository installable by
//! APM, `npx skills`, Claude plugin marketplaces, Copilot, Codex and Cursor.

use crate::config::{self, LicenseRecord, PublishTarget};
use crate::ctx::Ctx;
use crate::git::{self, git};
use crate::license::{self, Gate};
use crate::risk::RiskReport;
use crate::skill::{self, SkillDoc};
use crate::source_repo::{self, SourceRepo};
use anyhow::{Context, Result, bail};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Marketplace names Claude Code reserves.
pub const RESERVED_MARKETPLACE_NAMES: &[&str] = &[
    "claude-code-marketplace",
    "claude-code-plugins",
    "claude-plugins-official",
    "anthropic-marketplace",
    "anthropic-plugins",
    "agent-skills",
    "life-sciences",
    "knowledge-work-plugins",
];

#[derive(Debug, Clone, Default)]
pub struct PublishOptions {
    pub target: String,
    pub bump: Option<String>,
    pub dry_run: bool,
    pub push: bool,
    pub pr: bool,
    pub accept_copyleft: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateResult {
    pub name: String,
    /// pass | warn | fail
    pub status: String,
    pub details: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PublishReport {
    pub target: String,
    pub repo: String,
    pub skills: Vec<String>,
    pub previous_version: Option<String>,
    pub version: Option<String>,
    pub suggested_bump: Option<String>,
    pub gates: Vec<GateResult>,
    pub risk_diff: Vec<String>,
    pub changes: Vec<String>,
    pub changelog: String,
    pub commit: Option<String>,
    pub tag: Option<String>,
    pub pushed: bool,
    pub pr_url: Option<String>,
    pub dry_run: bool,
    pub blocked: bool,
}

fn gate(name: &str, status: &str, details: Vec<String>) -> GateResult {
    GateResult { name: name.into(), status: status.into(), details }
}

fn exclude_set(globs: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for g in globs {
        b.add(Glob::new(g).with_context(|| format!("invalid exclude glob `{g}`"))?);
        if let Some(rest) = g.strip_suffix("/**") {
            b.add(Glob::new(rest)?);
        }
    }
    Ok(b.build()?)
}

fn selected_skills(ws: &SourceRepo, t: &PublishTarget) -> Result<Vec<String>> {
    if t.skills.iter().any(|s| s == "*") {
        return Ok(ws.manifest.skills.keys().cloned().collect());
    }
    for s in &t.skills {
        ws.skill(s)?;
    }
    Ok(t.skills.clone())
}

/// The remote a publish target points at: `owner/repo` or `host/owner/repo` shorthand,
/// any git URL, or a path to a repository (relative to the source repo root).
fn target_url(ctx: &Ctx, ws: &SourceRepo, t: &PublishTarget) -> Result<String> {
    let r = t.repo.trim();
    if r.is_empty() {
        bail!("publish target has an empty `repo`");
    }
    if r.contains("://") || r.contains('@') {
        return Ok(r.to_string());
    }
    if r.starts_with('.') || r.starts_with('/') || r.starts_with('~') {
        let p = ctx.paths.expand(r);
        let p = if p.is_absolute() { p } else { ws.root.join(p) };
        return Ok(crate::paths::canon(&p).unwrap_or(p).to_string_lossy().to_string());
    }
    let src = crate::id::parse_source_input(r).with_context(|| format!("`{r}` is not a repository (use owner/repo or a git URL)"))?;
    Ok(git::clone_url(&src))
}

/// Default marketplace name: the last path segment of the target URL.
fn repo_name(url: &str) -> String {
    let tail = url.trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or(url);
    tail.strip_suffix(".git").unwrap_or(tail).to_string()
}

/// The clone New Tricks publishes through, reset to the remote's default branch.
/// Nothing is kept locally between runs: whatever the remote has is the starting point.
fn prepare_target(ctx: &Ctx, url: &str) -> Result<(PathBuf, String, std::fs::File)> {
    let key = crate::paths::path_key(Path::new(url));
    let lock =
        std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(ctx.paths.locks().join(format!("publish-{key}.lock")))?;
    lock.lock().context("locking publish target")?;
    let dir = ctx.paths.publish_clones().join(key);
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(dir.parent().unwrap())?;
        let _ = std::fs::remove_dir_all(&dir);
        git(dir.parent().unwrap(), &["clone", "-q", url, &dir.to_string_lossy()])
            .with_context(|| format!("cloning publish target {url}"))?;
    } else {
        git(&dir, &["remote", "set-url", "origin", url])?;
        git(&dir, &["fetch", "-q", "--prune", "--prune-tags", "--force", "--tags", "origin"])
            .with_context(|| format!("fetching publish target {url}"))?;
    }
    let branch = git::ls_remote(url)?.default_branch.unwrap_or_else(|| "main".into());
    if git::git_ok(&dir, &["rev-parse", "--verify", "-q", &format!("refs/remotes/origin/{branch}")]) {
        git(&dir, &["checkout", "-q", "-f", "-B", &branch, &format!("origin/{branch}")])?;
    } else {
        // Empty remote: start its first branch.
        git(&dir, &["checkout", "-q", "-f", "--orphan", &branch])?;
        let _ = git(&dir, &["rm", "-rfq", "--cached", "."]);
    }
    git(&dir, &["clean", "-qfdx"])?;
    Ok((dir, branch, lock))
}

/// Canonical source string for provenance: the `origin` remote, normalized. Published
/// repositories never carry a local path or credentials: without a usable remote this is
/// `local:<repo folder name>`.
pub fn source_label(dir: &Path) -> String {
    let local = || format!("local:{}", dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    let Ok(url) = git(dir, &["remote", "get-url", "origin"]) else { return local() };
    // A URL remote may carry credentials (`https://user:token@host/…`): drop them first.
    let Some((scheme, rest)) = url.split_once("://").filter(|(scheme, _)| *scheme != "file") else {
        // scp-like `git@host:owner/repo`, or a local path.
        return crate::id::parse_source_input(&url).map(|s| s.to_string()).unwrap_or_else(|_| local());
    };
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let url = format!("{scheme}://{host}/{path}");
    crate::id::parse_source_input(&url).map(|s| s.to_string()).unwrap_or(url)
}

fn target_visibility(ctx: &Ctx, repo: &Path) -> (bool, String) {
    let Ok(url) = git(repo, &["remote", "get-url", "origin"]) else {
        return (true, "no `origin` remote; treating the target as public".into());
    };
    let Ok(src) = crate::id::parse_source_input(&url) else {
        return (true, format!("cannot parse remote {url}; treating as public"));
    };
    if !src.is_github() || ctx.opts.offline {
        return (true, format!("cannot verify visibility of {src}; treating as public"));
    }
    #[derive(serde::Deserialize)]
    struct R {
        private: bool,
    }
    match ctx.gh.api_json::<R>(&src.host, &format!("/repos/{}", src.full_name())) {
        Ok(r) => (!r.private, format!("{src} is {}", if r.private { "private" } else { "public" })),
        Err(e) => (true, format!("cannot verify visibility of {src} ({e:#}); treating as public")),
    }
}

fn latest_version(repo: &Path) -> Option<semver::Version> {
    let tags = git(repo, &["tag", "--list"]).ok()?;
    tags.lines().filter_map(crate::resolve::parse_semver).filter(|v| v.pre.is_empty()).max()
}

fn bump_version(v: &semver::Version, kind: &str) -> Result<semver::Version> {
    Ok(match kind {
        "major" => semver::Version::new(v.major + 1, 0, 0),
        "minor" => semver::Version::new(v.major, v.minor + 1, 0),
        "patch" => semver::Version::new(v.major, v.minor, v.patch + 1),
        other => {
            let x = semver::Version::parse(other.trim_start_matches('v'))
                .with_context(|| format!("--bump must be major, minor, patch or a version, got `{other}`"))?;
            if &x <= v {
                bail!("version {x} is not greater than the current {v}");
            }
            x
        }
    })
}

fn rank(b: &str) -> u8 {
    match b {
        "major" => 3,
        "minor" => 2,
        _ => 1,
    }
}

fn file_set(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut m = BTreeMap::new();
    if !dir.exists() {
        return m;
    }
    for e in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| e.file_name() != ".git").flatten() {
        if e.file_type().is_file()
            && let Ok(b) = std::fs::read(e.path())
        {
            m.insert(e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"), b);
        }
    }
    m
}

fn headings(text: &str) -> BTreeSet<String> {
    text.lines().filter(|l| l.starts_with('#')).map(|l| l.trim().to_string()).collect()
}

fn similarity(a: &str, b: &str) -> f64 {
    let wa: BTreeSet<&str> = a.split_whitespace().collect();
    let wb: BTreeSet<&str> = b.split_whitespace().collect();
    let u = wa.union(&wb).count() as f64;
    if u == 0.0 { 1.0 } else { wa.intersection(&wb).count() as f64 / u }
}

/// Suggested bump for one skill: previously published copy → new staged copy.
pub fn suggest_bump(old: &Path, new: &Path) -> (String, Vec<String>) {
    if !old.exists() {
        return ("minor".into(), vec!["new skill".into()]);
    }
    let (fo, fnw) = (file_set(old), file_set(new));
    let mut reasons = Vec::new();
    let so = fo.get("SKILL.md").map(|b| SkillDoc::parse(&String::from_utf8_lossy(b))).unwrap_or_default();
    let sn = fnw.get("SKILL.md").map(|b| SkillDoc::parse(&String::from_utf8_lossy(b))).unwrap_or_default();
    let mut level = "patch";
    if so.name != sn.name {
        reasons.push("name changed".into());
        level = "major";
    }
    if similarity(so.description.as_deref().unwrap_or(""), sn.description.as_deref().unwrap_or("")) < 0.5 {
        reasons.push("description rewritten".into());
        level = "major";
    }
    let removed: Vec<&String> = fo.keys().filter(|k| !fnw.contains_key(*k)).collect();
    if !removed.is_empty() {
        reasons.push(format!("files removed: {}", removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        level = "major";
    }
    if crate::risk::tools_widened(so.allowed_tools.as_deref(), sn.allowed_tools.as_deref()) {
        reasons.push("allowed-tools widened".into());
        level = "major";
    }
    if level != "major" {
        let added: Vec<&String> = fnw.keys().filter(|k| !fo.contains_key(*k)).collect();
        let new_sections = headings(&sn.body).difference(&headings(&so.body)).count();
        if !added.is_empty() || new_sections > 0 {
            level = "minor";
            if !added.is_empty() {
                reasons.push(format!("files added: {}", added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
            }
            if new_sections > 0 {
                reasons.push(format!("{new_sections} new section(s)"));
            }
        } else if fo != fnw {
            reasons.push("prose changes".into());
        }
    }
    (level.into(), reasons)
}

/// Last source repo commit published to this target (from the trailer).
fn last_published_source(repo: &Path) -> Option<String> {
    let log = git(repo, &["log", "-50", "--format=%(trailers:key=Tricks-Source,valueonly)"]).ok()?;
    log.lines().find(|l| !l.trim().is_empty()).and_then(|l| l.rsplit_once('@').map(|(_, c)| c.trim().to_string()))
}

fn changelog_section(ws: &SourceRepo, skills: &[String], since: Option<&str>, heading: &str) -> String {
    let mut out = format!("## {heading}\n\n");
    let mut any = false;
    for name in skills {
        let Ok(s) = ws.skill(name) else { continue };
        let range = match since {
            Some(c) if git::git_ok(&ws.root, &["cat-file", "-e", &format!("{c}^{{commit}}")]) => format!("{c}..HEAD"),
            _ => "HEAD".into(),
        };
        let log = git(&ws.root, &["log", "--no-merges", "--format=%s", &range, "--", &s.path]).unwrap_or_default();
        let subjects: Vec<&str> = log.lines().filter(|l| !l.trim().is_empty()).take(30).collect();
        if subjects.is_empty() {
            continue;
        }
        any = true;
        out.push_str(&format!("### {name}\n\n"));
        for s in subjects {
            out.push_str(&format!("- {s}\n"));
        }
        out.push('\n');
    }
    if !any {
        out.push_str("- No skill changes.\n\n");
    }
    out
}

fn marketplace_json(
    ws: &SourceRepo,
    t: &PublishTarget,
    market_name: &str,
    skills: &[String],
    version: Option<&str>,
) -> Result<serde_json::Value> {
    let owner = t.owner.clone().unwrap_or_else(|| git(&ws.root, &["config", "user.name"]).unwrap_or_else(|_| ws.name.clone()));
    let desc = t.description.clone().unwrap_or_else(|| format!("Agent skills published from {}", ws.name));
    let mut plugins = Vec::new();
    let mut grouped: BTreeSet<String> = BTreeSet::new();
    for (plugin, members) in &t.plugins {
        let mut paths = Vec::new();
        for m in members {
            if !skills.contains(m) {
                bail!("plugin group `{plugin}` lists `{m}`, which is not published to this target");
            }
            grouped.insert(m.clone());
            paths.push(format!("./skills/{m}"));
        }
        let mut p = serde_json::json!({ "name": plugin, "source": "./", "strict": false, "description": format!("{} ({})", desc, members.join(", ")), "skills": paths });
        if let Some(v) = version {
            p["version"] = serde_json::json!(v);
        }
        plugins.push(p);
    }
    let rest: Vec<&String> = skills.iter().filter(|s| !grouped.contains(*s)).collect();
    if !rest.is_empty() {
        let mut p = serde_json::json!({ "name": market_name, "source": "./", "strict": false, "description": desc });
        if !t.plugins.is_empty() {
            p["skills"] = serde_json::json!(rest.iter().map(|s| format!("./skills/{s}")).collect::<Vec<_>>());
        }
        if let Some(v) = version {
            p["version"] = serde_json::json!(v);
        }
        plugins.insert(0, p);
    }
    Ok(serde_json::json!({ "name": market_name, "owner": { "name": owner }, "description": desc, "plugins": plugins }))
}

fn apm_yml(market_name: &str, desc: &str, version: &str, license: Option<&str>) -> String {
    let d: String = desc.chars().take(80).collect();
    let mut s = format!(
        "# Generated by New Tricks. APM installs skills/<name>/SKILL.md directly; this file adds package metadata.\nname: {market_name}\nversion: {version}\ndescription: \"{}\"\n",
        d.replace('"', "'")
    );
    if let Some(l) = license {
        s.push_str(&format!("license: {l}\n"));
    }
    s
}

pub fn validate_marketplace_name(n: &str) -> Result<()> {
    if !crate::id::valid_skill_name(n) {
        bail!("marketplace name `{n}` must be kebab-case (set publish.targets.<t>.marketplace)");
    }
    if RESERVED_MARKETPLACE_NAMES.contains(&n) {
        bail!("marketplace name `{n}` is reserved by Claude Code; set publish.targets.<t>.marketplace to another name");
    }
    Ok(())
}

pub fn publish(ctx: &Ctx, opts: &PublishOptions) -> Result<PublishReport> {
    let ws = source_repo::require(ctx)?;
    let _lock = source_repo::source_repo_lock(ctx, &ws)?;
    let t = ws.manifest.publish.targets.get(&opts.target).cloned().with_context(|| {
        format!(
            "no publish target `{}` (have: {})",
            opts.target,
            ws.manifest.publish.targets.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;
    if !opts.dry_run && !opts.push && !opts.pr {
        bail!(
            "publishing to `{}` needs --push (commit, tag and push) or --pr (push a branch and open a pull request); preview with --dry-run",
            opts.target
        );
    }
    let url = target_url(ctx, &ws, &t)?;
    let skills = selected_skills(&ws, &t)?;
    let market_name = t.marketplace.clone().unwrap_or_else(|| repo_name(&url));
    validate_marketplace_name(&market_name)?;
    let excludes = exclude_set(&t.exclude)?;
    let mut gates = Vec::new();
    let mut blocked = false;

    // 1. Committed source.
    let dirty = git(&ws.root, &["status", "--porcelain"])?;
    if dirty.trim().is_empty() {
        gates.push(gate("committed source", "pass", vec![]));
    } else if opts.dry_run {
        gates.push(gate("committed source", "warn", vec!["source repo has uncommitted changes (allowed for --dry-run)".into()]));
    } else {
        gates.push(gate("committed source", "fail", dirty.lines().take(10).map(String::from).collect()));
        blocked = true;
    }
    let source_commit = git::head_commit(&ws.root)?;

    // 2. Lint.
    let lint = crate::lint::lint_repo(ctx, &ws, &skills)?;
    if lint.errors > 0 {
        blocked = true;
        gates.push(gate(
            "lint",
            "fail",
            lint.findings
                .iter()
                .filter(|f| f.severity == "error")
                .map(|f| format!("{} {}:{} {}", f.code, f.skill, f.file, f.message))
                .collect(),
        ));
    } else {
        gates.push(gate(
            "lint",
            if lint.warnings > 0 { "warn" } else { "pass" },
            lint.findings.iter().filter(|f| f.severity == "warning").map(|f| format!("{} {} {}", f.code, f.skill, f.message)).collect(),
        ));
    }

    let (repo, branch, _target_lock) = prepare_target(ctx, &url)?;

    // 3. Licence gate for vendored skills.
    let (public, vis_note) = target_visibility(ctx, &repo);
    let mut lic_details = vec![vis_note];
    let mut lic_status = "pass";
    let mut licenses: BTreeMap<String, LicenseRecord> = BTreeMap::new();
    for name in &skills {
        let s = ws.skill(name)?;
        let dir = ws.root.join(&s.path);
        let rec = license::detect(&crate::inspect::gather_dir(&dir));
        let rec = match (&s.upstream, ws.lock.skills.get(name).and_then(|l| l.license.clone())) {
            (Some(_), Some(locked)) if license::Class::parse(&locked.class) > license::Class::parse(&rec.class) => locked,
            _ => rec,
        };
        licenses.insert(name.clone(), rec.clone());
        if s.upstream.is_none() {
            continue; // your own work
        }
        match license::gate(&rec, public, opts.accept_copyleft, s.license_override.is_some()) {
            Gate::Allow => {}
            Gate::Warn(m) => {
                if lic_status == "pass" {
                    lic_status = "warn";
                }
                lic_details.push(format!("{name}: {m}"));
            }
            Gate::Block(m) => {
                lic_status = "fail";
                blocked = true;
                lic_details.push(format!("{name}: {m}"));
                if s.license_override.is_none() {
                    lic_details.push(format!(
                        "{name}: if you have permission (e.g. a separate agreement), record it in tricks.toml: \
                         [skills.{name}] license-override = {{ justification = \"…\" }}"
                    ));
                }
            }
        }
    }
    gates.push(gate("licence", lic_status, lic_details));

    // Stage the output.
    let staging = tempfile::tempdir()?;
    let stage = staging.path();
    let mut leak_fail = Vec::new();
    let mut leak_warn = Vec::new();
    for name in &skills {
        let src = ws.root.join(&ws.skill(name)?.path);
        let dest = stage.join("skills").join(name);
        for e in walkdir::WalkDir::new(&src).into_iter().filter_entry(|e| e.file_name() != ".git").flatten() {
            if !e.file_type().is_file() {
                continue;
            }
            let rel = e.path().strip_prefix(&src).unwrap().to_string_lossy().replace('\\', "/");
            if excludes.is_match(&rel) || rel.ends_with(crate::merge::UPSTREAM_SUFFIX) {
                continue;
            }
            let lower = rel.to_lowercase();
            if lower.ends_with(".env") || lower.starts_with("notes/") || lower.contains(".draft.") || lower.ends_with(".ds_store") {
                leak_warn.push(format!("{name}/{rel} looks private to the source repo (add it to exclude)"));
            }
            let to = dest.join(&rel);
            std::fs::create_dir_all(to.parent().unwrap())?;
            std::fs::copy(e.path(), &to)?;
        }
        let r = RiskReport::scan_dir(&dest);
        for f in &r.secrets {
            leak_fail.push(format!("{name}/{}:{} {}", f.file, f.line, f.detail));
        }
    }
    if !leak_fail.is_empty() {
        blocked = true;
        gates.push(gate("leak check", "fail", leak_fail.into_iter().chain(leak_warn).collect()));
    } else {
        gates.push(gate("leak check", if leak_warn.is_empty() { "pass" } else { "warn" }, leak_warn));
    }

    // Version and suggested bump (against the currently published copy).
    let prev = latest_version(&repo);
    let mut suggested = "patch".to_string();
    let mut risk_diff = Vec::new();
    let old_owned: BTreeSet<String> = std::fs::read_to_string(repo.join(config::PUBLISHED_FILE))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(String::from)
        .collect();
    for name in &skills {
        let old = repo.join("skills").join(name);
        let new = stage.join("skills").join(name);
        let (b, reasons) = suggest_bump(&old, &new);
        if rank(&b) > rank(&suggested) {
            suggested = b.clone();
        }
        for r in reasons.iter().filter(|r| *r != "prose changes") {
            risk_diff.push(format!("{name}: {r}"));
        }
        let rd = RiskReport::scan_dir(&new).diff_from(&RiskReport::scan_dir(&old));
        risk_diff.extend(rd.into_iter().map(|l| format!("{name}: {l}")));
    }
    for owned in &old_owned {
        if let Some(n) = owned.strip_prefix("skills/")
            && !skills.iter().any(|s| s == n)
        {
            suggested = "major".into();
            risk_diff.push(format!("{n}: removed from target"));
        }
    }
    let version = match &opts.bump {
        Some(b) => Some(bump_version(&prev.clone().unwrap_or(semver::Version::new(0, 0, 0)), b)?),
        None => None,
    };
    let version_s = version.as_ref().map(|v| v.to_string());

    // Transform SKILL.md: strip tricks-only keys, set metadata.version.
    for name in &skills {
        let p = stage.join("skills").join(name).join("SKILL.md");
        if let Ok(t0) = std::fs::read_to_string(&p) {
            let mut t1 = skill::strip_metadata_prefix(&t0, "tricks-");
            if let Some(v) = &version_s {
                t1 = skill::set_metadata_value(&t1, "version", v);
            }
            if t1 != t0 {
                std::fs::write(&p, t1)?;
            }
        }
    }

    // Generated files.
    let heading = match &version_s {
        Some(v) => format!("v{v} ({})", today()),
        None => format!("Unreleased ({})", today()),
    };
    let since = last_published_source(&repo);
    let section = changelog_section(&ws, &skills, since.as_deref(), &heading);
    let old_changelog = std::fs::read_to_string(repo.join("CHANGELOG.md")).unwrap_or_else(|_| "# Changelog\n\n".into());
    let changelog = match old_changelog.split_once("\n## ") {
        Some((head, rest)) => format!("{}\n{}## {}", head.trim_end(), section, rest),
        None => format!("{}\n\n{}", old_changelog.trim_end(), section),
    };
    std::fs::write(stage.join("CHANGELOG.md"), &changelog)?;
    let mp = marketplace_json(&ws, &t, &market_name, &skills, version_s.as_deref())?;
    std::fs::create_dir_all(stage.join(".claude-plugin"))?;
    std::fs::write(stage.join(".claude-plugin/marketplace.json"), serde_json::to_string_pretty(&mp)? + "\n")?;
    let apm_version = version_s.clone().or_else(|| prev.as_ref().map(|v| v.to_string())).unwrap_or_else(|| "0.0.0".into());
    let root_license = ["LICENSE", "LICENSE.md", "LICENSE.txt"].iter().find_map(|f| std::fs::read_to_string(ws.root.join(f)).ok());
    let root_spdx = root_license.as_deref().and_then(|t| license::detect_text(t).spdx);
    std::fs::write(
        stage.join("apm.yml"),
        apm_yml(&market_name, mp["description"].as_str().unwrap_or(""), &apm_version, root_spdx.as_deref()),
    )?;
    let mut prov = String::from("# Provenance\n\nGenerated by New Tricks. Skills below were published from ");
    prov.push_str(&format!(
        "`{}` at commit `{}`.\n\n| Skill | Upstream | Base commit | Licence |\n|---|---|---|---|\n",
        source_label(&ws.root),
        &source_commit[..12]
    ));
    for name in &skills {
        let s = ws.skill(name)?;
        let base = ws
            .lock
            .skills
            .get(name)
            .and_then(|l| l.base.clone())
            .map(|b| format!("`{}`", &b[..12.min(b.len())]))
            .unwrap_or_else(|| "—".into());
        let lic =
            licenses.get(name).map(|l| format!("{} ({})", l.spdx.clone().unwrap_or_else(|| "none".into()), l.class)).unwrap_or_default();
        prov.push_str(&format!("| {name} | {} | {base} | {lic} |\n", s.upstream.clone().unwrap_or_else(|| "original".into())));
    }
    std::fs::write(stage.join("PROVENANCE.md"), prov)?;
    if let Some(l) = &root_license {
        std::fs::write(stage.join("LICENSE"), l)?;
    }

    let mut owned: BTreeSet<String> = skills.iter().map(|s| format!("skills/{s}")).collect();
    for f in ["CHANGELOG.md", ".claude-plugin/marketplace.json", "apm.yml", "PROVENANCE.md"] {
        owned.insert(f.into());
    }
    if root_license.is_some() {
        owned.insert("LICENSE".into());
    }
    std::fs::write(
        stage.join(config::PUBLISHED_FILE),
        format!(
            "# Paths owned by New Tricks; re-publish replaces exactly these.\n{}\n",
            owned.iter().cloned().collect::<Vec<_>>().join("\n")
        ),
    )?;
    owned.insert(config::PUBLISHED_FILE.into());

    // Target file changes (owned paths only).
    let mut changes = Vec::new();
    let staged_files = file_set(stage);
    let mut target_files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for o in owned.iter().chain(old_owned.iter()) {
        let p = repo.join(o);
        if p.is_dir() {
            for (k, v) in file_set(&p) {
                target_files.insert(format!("{o}/{k}"), v);
            }
        } else if let Ok(b) = std::fs::read(&p) {
            target_files.insert(o.clone(), b);
        }
    }
    for (k, v) in &staged_files {
        match target_files.get(k) {
            None => changes.push(format!("A {k}")),
            Some(o) if o != v => changes.push(format!("M {k}")),
            _ => {}
        }
    }
    for k in target_files.keys() {
        if !staged_files.contains_key(k) {
            changes.push(format!("D {k}"));
        }
    }

    if !risk_diff.is_empty() {
        gates.push(gate("risk diff", "warn", risk_diff.clone()));
    } else {
        gates.push(gate("risk diff", "pass", vec![]));
    }

    let mut rep = PublishReport {
        target: opts.target.clone(),
        repo: url.clone(),
        skills: skills.clone(),
        previous_version: prev.map(|v| v.to_string()),
        version: version_s.clone(),
        suggested_bump: Some(suggested),
        gates,
        risk_diff: risk_diff.clone(),
        changes: changes.clone(),
        changelog: section,
        commit: None,
        tag: None,
        pushed: false,
        pr_url: None,
        dry_run: opts.dry_run,
        blocked,
    };
    if opts.dry_run || blocked {
        return Ok(rep);
    }
    if changes.is_empty() {
        return Ok(rep);
    }
    if !risk_diff.is_empty() && !ctx.confirm(&format!("Publish to {} with these changes?", opts.target), &risk_diff)? {
        bail!("cancelled");
    }

    // Write: replace owned paths only.
    let journal = ctx.state.journal_start("publish", &format!("{} → {url}", ws.root.display()))?;
    if opts.pr {
        let branch = format!("tricks/publish-{}", version_s.clone().unwrap_or_else(|| source_commit[..9].to_string()));
        git(&repo, &["checkout", "-q", "-B", &branch])?;
    }
    for o in owned.iter().chain(old_owned.iter()) {
        crate::deploy::remove_path(&repo.join(o))?;
    }
    for (k, v) in &staged_files {
        let p = repo.join(k);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, v)?;
        #[cfg(unix)]
        {
            let src_exec = crate::risk::is_exec(&stage.join(k));
            if src_exec {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755))?;
            }
        }
    }
    let mut add = vec!["add", "-A", "--"];
    let owned_list: Vec<String> = owned.iter().chain(old_owned.iter()).cloned().collect();
    add.extend(owned_list.iter().map(|s| s.as_str()));
    git(&repo, &add)?;
    let msg = match &version_s {
        Some(v) => format!("Publish v{v} from {}", ws.name),
        None => format!("Publish from {}", ws.name),
    };
    let trailer = format!("Tricks-Source: {}@{}", source_label(&ws.root), source_commit);
    git(&repo, &["commit", "-q", "-m", &msg, "--trailer", &trailer])?;
    rep.commit = Some(git::head_commit(&repo)?);
    if let Some(v) = &version_s
        && !opts.pr
    {
        let tag = format!("v{v}");
        git(&repo, &["tag", "-a", &tag, "-m", &format!("{} {tag}", ws.name)])?;
        rep.tag = Some(tag);
    }
    let head = git::current_branch(&repo).unwrap_or_else(|| branch.clone());
    git(&repo, &["push", "-q", "-u", "origin", &head]).with_context(|| format!("pushing to {url}"))?;
    if let Some(tag) = &rep.tag {
        git(&repo, &["push", "-q", "origin", tag])?;
    }
    rep.pushed = true;
    if opts.pr {
        let body = format!(
            "Published by New Tricks from `{}` at `{}`.\n\n{}\n\nTag `v{}` after merging.",
            source_label(&ws.root),
            &source_commit[..12],
            rep.changelog,
            version_s.clone().unwrap_or_default()
        );
        let out = std::process::Command::new("gh")
            .args(["pr", "create", "--title", &msg, "--body", &body])
            .current_dir(&repo)
            .output()
            .context("running gh pr create")?;
        if !out.status.success() {
            bail!("gh pr create failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        rep.pr_url = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    ctx.state.journal_finish(journal, "done")?;
    Ok(rep)
}

fn today() -> String {
    let f = time::macros::format_description!("[year]-[month]-[day]");
    time::OffsetDateTime::now_utc().format(&f).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bumps() {
        let v = semver::Version::new(1, 2, 3);
        assert_eq!(bump_version(&v, "minor").unwrap().to_string(), "1.3.0");
        assert_eq!(bump_version(&v, "major").unwrap().to_string(), "2.0.0");
        assert_eq!(bump_version(&v, "v1.2.4").unwrap().to_string(), "1.2.4");
        assert!(bump_version(&v, "1.0.0").is_err());
    }

    #[test]
    fn marketplace_names() {
        assert!(validate_marketplace_name("acme-skills").is_ok());
        assert!(validate_marketplace_name("agent-skills").is_err());
        assert!(validate_marketplace_name("Acme").is_err());
    }

    #[test]
    fn source_label_never_publishes_local_paths_or_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("my-skills");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]).unwrap();
        assert_eq!(source_label(&dir), "local:my-skills");
        git(&dir, &["remote", "add", "origin", "/srv/git/my-skills.git"]).unwrap();
        assert_eq!(source_label(&dir), "local:my-skills");
        git(&dir, &["remote", "set-url", "origin", "https://github.com/acme/my-skills.git"]).unwrap();
        assert_eq!(source_label(&dir), "github.com/acme/my-skills");
        git(&dir, &["remote", "set-url", "origin", "https://user:secret@git.example.internal:8443/team/my-skills.git"]).unwrap();
        let l = source_label(&dir);
        assert!(!l.contains("secret") && !l.contains("user@"), "{l}");
        git(&dir, &["remote", "set-url", "origin", "https://x-access-token:ghp_abc@github.com/acme/my-skills.git"]).unwrap();
        assert_eq!(source_label(&dir), "github.com/acme/my-skills");
        git(&dir, &["remote", "set-url", "origin", "git@github.com:acme/my-skills.git"]).unwrap();
        assert_eq!(source_label(&dir), "github.com/acme/my-skills");
    }
}
