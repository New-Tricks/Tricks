//! Catalog adapters (spec §7). Catalog adapters supply pointers; the git adapter
//! resolves every pointer to real content. Modes: indexed (ahead of time) and
//! live-query (at search time, cached with a TTL).

use crate::config::{self, CatalogEntry};
use crate::ctx::Ctx;
use crate::git::host_map;
use crate::github::{GitHub, encode_query, is_not_found};
use crate::id::{SourceId, parse_source_input};
use crate::index::{self, IndexedSkill};
use crate::resolve::{Fetch, open_mirror};
use crate::state::now;
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DEFAULT_REPOS: &[&str] = &[
    "github.com/anthropics/skills",
    "github.com/openai/skills",
    "github.com/vercel-labs/agent-skills",
    "github.com/github/awesome-copilot",
    "github.com/obra/superpowers",
];

/// Maximum external repositories a marketplace refresh will index.
const MARKETPLACE_REPO_CAP: usize = 60;
const LIVE_TTL_SECS: i64 = 6 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Repo,
    Marketplace,
    WellKnown,
    Pointers,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Kind> {
        Ok(match s {
            "repo" => Kind::Repo,
            "marketplace" => Kind::Marketplace,
            "wellknown" | "well-known" => Kind::WellKnown,
            "pointers" => Kind::Pointers,
            _ => bail!("unknown catalog kind `{s}` (repo | marketplace | wellknown | pointers)"),
        })
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Repo => "repo",
            Kind::Marketplace => "marketplace",
            Kind::WellKnown => "wellknown",
            Kind::Pointers => "pointers",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Catalog {
    pub key: String,
    pub kind: Kind,
    pub default: bool,
    pub disabled: bool,
    pub last_indexed: Option<i64>,
    pub skills: i64,
}

fn classify_key(key: &str, entry: &CatalogEntry) -> Result<Kind> {
    if let Some(k) = &entry.kind {
        return Kind::parse(k);
    }
    if key.ends_with("apm.yml") || key.ends_with("skills-lock.json") {
        return Ok(Kind::Pointers);
    }
    if key.starts_with("https://") && !key.contains("github") {
        return Ok(Kind::WellKnown);
    }
    Ok(Kind::Repo)
}

/// All configured catalogs (user config + defaults).
pub fn list(ctx: &Ctx) -> Result<Vec<Catalog>> {
    let m = crate::user::load_manifest(ctx)?;
    let mut out: BTreeMap<String, Catalog> = BTreeMap::new();
    if m.settings.default_catalogs {
        for r in DEFAULT_REPOS {
            out.insert(
                r.to_string(),
                Catalog { key: r.to_string(), kind: Kind::Repo, default: true, disabled: false, last_indexed: None, skills: 0 },
            );
        }
    }
    for (k, e) in &m.catalogs {
        let kind = classify_key(k, e)?;
        out.insert(k.clone(), Catalog { key: k.clone(), kind, default: false, disabled: e.disabled, last_indexed: None, skills: 0 });
    }
    for s in out.values_mut() {
        s.last_indexed = ctx.state.fetched_at(&format!("index:{}", s.key))?;
        s.skills = ctx.state.conn.query_row("SELECT COUNT(*) FROM skills WHERE origin=?1 OR source=?1", [&s.key], |r| r.get(0))?;
    }
    Ok(out.into_values().collect())
}

/// Normalize user input into a catalog key and kind.
pub fn normalize_input(ctx: &Ctx, input: &str, kind: Option<&str>) -> Result<(String, Kind)> {
    let p = std::path::Path::new(input);
    let looks_file =
        input.ends_with("apm.yml") || input.ends_with("skills-lock.json") || input.ends_with(".json") || input.ends_with(".yml");
    if looks_file && (p.exists() || ctx.opts.cwd.join(p).exists()) {
        let abs = if p.is_absolute() { p.to_path_buf() } else { ctx.opts.cwd.join(p) };
        let abs = crate::paths::canon(&abs).unwrap_or(abs);
        return Ok((ctx.paths.contract(&abs), Kind::Pointers));
    }
    if let Some(k) = kind {
        let k = Kind::parse(k)?;
        if k == Kind::WellKnown {
            let url = input.trim_end_matches('/');
            let url = if url.starts_with("http") { url.to_string() } else { format!("https://{url}") };
            return Ok((url, k));
        }
        let src = parse_source_input(input)?;
        return Ok((src.to_string(), k));
    }
    if input.starts_with("https://") && !input.contains("github") {
        return Ok((input.trim_end_matches('/').to_string(), Kind::WellKnown));
    }
    let src = parse_source_input(input)?;
    Ok((src.to_string(), Kind::Repo))
}

pub fn add(ctx: &Ctx, input: &str, kind: Option<&str>) -> Result<(String, Kind)> {
    let (key, k) = normalize_input(ctx, input, kind)?;
    let path = ctx.paths.user_manifest();
    let mut doc = config::load_doc(&path)?;
    let entry = CatalogEntry { kind: if k == Kind::Repo { None } else { Some(k.as_str().into()) }, url: None, disabled: false };
    config::table_mut(&mut doc, &["catalogs"]).insert(&key, config::to_inline(&entry)?);
    config::save_doc(&path, &doc)?;
    Ok((key, k))
}

pub fn remove(ctx: &Ctx, input: &str) -> Result<String> {
    let m = crate::user::load_manifest(ctx)?;
    let key = if m.catalogs.contains_key(input) {
        input.to_string()
    } else {
        normalize_input(ctx, input, None).map(|(k, _)| k).unwrap_or_else(|_| input.to_string())
    };
    let path = ctx.paths.user_manifest();
    let mut doc = config::load_doc(&path)?;
    if DEFAULT_REPOS.contains(&key.as_str()) && !m.catalogs.contains_key(&key) {
        // Disable a default catalog.
        let entry = CatalogEntry { disabled: true, ..Default::default() };
        config::table_mut(&mut doc, &["catalogs"]).insert(&key, config::to_inline(&entry)?);
    } else if config::table_mut(&mut doc, &["catalogs"]).remove(&key).is_none() {
        bail!("no catalog `{input}`");
    }
    config::save_doc(&path, &doc)?;
    // Drop index rows that came only from this catalog.
    let origin_ids: BTreeSet<String> = BTreeSet::new();
    let _ = index::prune_source(ctx, &key, &origin_ids);
    ctx.state.conn.execute("DELETE FROM listings WHERE catalog=?1", [format!("{}:{key}", kind_label(&key))])?;
    Ok(key)
}

fn kind_label(key: &str) -> &'static str {
    if key.ends_with("apm.yml") || key.ends_with("skills-lock.json") { "pointers" } else { "marketplace" }
}

#[derive(Debug, Default, Serialize)]
pub struct RefreshReport {
    pub indexed: Vec<(String, usize)>,
    pub errors: Vec<(String, String)>,
    pub skipped: usize,
}

/// Refresh stale (or all, with `force`) indexed catalogs.
pub fn refresh(ctx: &Ctx, force: bool, only: Option<&str>) -> Result<RefreshReport> {
    let mut rep = RefreshReport::default();
    if ctx.opts.offline {
        return Ok(rep);
    }
    let interval = crate::user::load_manifest(ctx)?.fetch_interval();
    // Plain repositories are fetched in parallel; catalogs one by one.
    let mut repo_jobs = Vec::new();
    let mut rest = Vec::new();
    for s in list(ctx)? {
        if s.disabled || only.map(|o| o != s.key).unwrap_or(false) {
            continue;
        }
        if !force && !ctx.state.is_stale(&format!("index:{}", s.key), interval)? {
            rep.skipped += 1;
            continue;
        }
        if s.kind == Kind::Repo
            && let Ok(src) = parse_source_input(&s.key)
        {
            ctx.ui.info(&format!("indexing {}", s.key));
            repo_jobs.push((src, None, s.key.clone()));
            continue;
        }
        rest.push(s);
    }
    for (src, r) in index_repos(ctx, &repo_jobs) {
        let key = src.to_string();
        match r {
            Ok(n) => {
                ctx.state.mark_fetched(&format!("index:{key}"))?;
                rep.indexed.push((key, n));
            }
            Err(e) => {
                ctx.ui.warn(&format!("{key}: {e:#}"));
                rep.errors.push((key, format!("{e:#}")));
            }
        }
    }
    for s in rest {
        if let Some(o) = only
            && s.key != o
        {
            continue;
        }
        let key = format!("index:{}", s.key);
        if !force && !ctx.state.is_stale(&key, interval)? {
            rep.skipped += 1;
            continue;
        }
        ctx.ui.info(&format!("indexing {} ({})", s.key, s.kind.as_str()));
        let r = match s.kind {
            Kind::Repo => parse_source_input(&s.key).and_then(|src| index_repo(ctx, &src, None, &s.key)),
            Kind::Marketplace => index_marketplace(ctx, &s.key),
            Kind::WellKnown => index_wellknown(ctx, &s.key),
            Kind::Pointers => index_pointers(ctx, &s.key),
        };
        match r {
            Ok(n) => {
                ctx.state.mark_fetched(&key)?;
                rep.indexed.push((s.key.clone(), n));
            }
            Err(e) => {
                ctx.ui.warn(&format!("{}: {e:#}", s.key));
                rep.errors.push((s.key.clone(), format!("{e:#}")));
            }
        }
    }
    Ok(rep)
}

// ---------------------------------------------------------------- git adapter

#[derive(Deserialize)]
struct RepoInfo {
    default_branch: String,
    #[serde(default)]
    stargazers_count: Option<i64>,
    #[serde(default)]
    license: Option<RepoLicense>,
}

#[derive(Deserialize)]
struct RepoLicense {
    spdx_id: Option<String>,
}

#[derive(Deserialize)]
struct CommitInfo {
    sha: String,
    commit: CommitDetail,
}

#[derive(Deserialize)]
struct CommitDetail {
    committer: Option<CommitPerson>,
}

#[derive(Deserialize)]
struct CommitPerson {
    date: Option<String>,
}

#[derive(Deserialize)]
struct Tree {
    sha: String,
    tree: Vec<TreeEntry>,
    #[serde(default)]
    truncated: bool,
}

#[derive(Deserialize, Clone)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    sha: String,
}

fn use_api(src: &SourceId) -> bool {
    src.is_github() && host_map(&src.host).is_none() && std::env::var_os("TRICKS_NO_API").is_none()
}

fn matches_filter(dir: &str, filter: Option<&[String]>) -> bool {
    match filter {
        None => true,
        Some(f) => f.iter().any(|p| {
            let p = p.trim_matches('/');
            p.is_empty() || p == "." || dir == p || dir.starts_with(&format!("{p}/"))
        }),
    }
}

/// Index skills of a repository at its default branch head. `filter` restricts to
/// path prefixes (catalog pointers). Returns the number of skills indexed.
pub fn index_repo(ctx: &Ctx, src: &SourceId, filter: Option<&[String]>, origin: &str) -> Result<usize> {
    let sink = ctx.ui_warn_sink();
    let fetched = if use_api(src) { fetch_github(&ctx.gh, src, filter, origin, &sink)? } else { fetch_repo_git(ctx, src, filter, origin)? };
    for w in sink.lock().unwrap().drain(..) {
        ctx.ui.warn(&w);
    }
    store_fetch(ctx, src, filter, fetched)
}

/// Index several repositories, fetching GitHub ones in parallel.
pub fn index_repos(ctx: &Ctx, repos: &[(SourceId, Option<Vec<String>>, String)]) -> Vec<(SourceId, Result<usize>)> {
    let (api, other): (Vec<_>, Vec<_>) = repos.iter().cloned().partition(|(s, _, _)| use_api(s));
    let sink = ctx.ui_warn_sink();
    let gh = &ctx.gh;
    let fetched = parallel_map(api, 6, |(src, filter, origin)| {
        let r = fetch_github(gh, &src, filter.as_deref(), &origin, &sink);
        (src, filter, r)
    });
    let mut out = Vec::new();
    for (src, filter, r) in fetched {
        let res = r.and_then(|f| store_fetch(ctx, &src, filter.as_deref(), f));
        out.push((src, res));
    }
    for (src, filter, origin) in other {
        let res = index_repo(ctx, &src, filter.as_deref(), &origin);
        out.push((src, res));
    }
    for w in sink.lock().unwrap().drain(..) {
        ctx.ui.warn(&w);
    }
    out
}

pub struct RepoFetch {
    pub skills: Vec<IndexedSkill>,
    pub stars: Option<i64>,
    pub default_branch: String,
    pub license: Option<String>,
}

fn store_fetch(ctx: &Ctx, src: &SourceId, filter: Option<&[String]>, f: RepoFetch) -> Result<usize> {
    let tx = ctx.state.conn.unchecked_transaction()?;
    ctx.state.conn.execute(
        "INSERT OR REPLACE INTO repo_info(source, stars, default_branch, license, fetched_at) VALUES(?1,?2,?3,?4,?5)",
        params![src.to_string(), f.stars, f.default_branch, f.license, now()],
    )?;
    let keep: BTreeSet<String> = f.skills.iter().map(|s| s.id.clone()).collect();
    for s in &f.skills {
        index::upsert(ctx, s)?;
    }
    if filter.is_none() {
        index::prune_source(ctx, &src.to_string(), &keep)?;
    }
    tx.commit()?;
    Ok(f.skills.len())
}

fn parse_date(s: &str) -> Option<i64> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok().map(|t| t.unix_timestamp())
}

pub type WarnSink = std::sync::Mutex<Vec<String>>;

fn fetch_github(gh: &GitHub, src: &SourceId, filter: Option<&[String]>, origin: &str, warn: &WarnSink) -> Result<RepoFetch> {
    let host = &src.host;
    let info: RepoInfo = gh.api_json(host, &format!("/repos/{}", src.full_name())).with_context(|| format!("reading {src}"))?;
    let repo_license = info.license.as_ref().and_then(|l| l.spdx_id.clone());
    let commit: CommitInfo =
        gh.api_json(host, &format!("/repos/{}/commits/{}", src.full_name(), crate::github::encode_path(&info.default_branch)))?;
    let updated = commit.commit.committer.and_then(|c| c.date).and_then(|d| parse_date(&d));
    let tree: Tree = gh.api_json(host, &format!("/repos/{}/git/trees/{}?recursive=1", src.full_name(), commit.sha))?;
    if tree.truncated {
        warn.lock().unwrap().push(format!("{src}: tree listing truncated by GitHub; some skills may be missing"));
    }
    let mut trees: BTreeMap<String, String> = BTreeMap::new();
    trees.insert(".".into(), tree.sha.clone());
    let mut dirs = Vec::new();
    for e in &tree.tree {
        if e.kind == "tree" {
            trees.insert(e.path.clone(), e.sha.clone());
        } else if e.kind == "blob" && (e.path == "SKILL.md" || e.path.ends_with("/SKILL.md")) {
            let d = e.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_else(|| ".".into());
            if matches_filter(&d, filter) && !d.split('/').any(|seg| seg == "node_modules") {
                dirs.push(d);
            }
        }
    }
    let entries = &tree.tree;
    let files_of = |dir: &str| -> Vec<(String, bool)> {
        let prefix = if dir == "." { String::new() } else { format!("{dir}/") };
        entries
            .iter()
            .filter(|e| e.kind == "blob" && e.path.starts_with(&prefix))
            .map(|e| (e.path[prefix.len()..].to_string(), e.mode == "100755"))
            .collect()
    };
    let fetched = parallel_map(dirs.clone(), 8, |d| {
        let p = if d == "." { "SKILL.md".to_string() } else { format!("{d}/SKILL.md") };
        gh.raw_file(host, src.full_name(), &commit.sha, &p).ok().flatten()
    });
    let mut out = Vec::new();
    for (d, content) in dirs.into_iter().zip(fetched) {
        let Some(bytes) = content else { continue };
        let text = String::from_utf8_lossy(&bytes);
        out.push(IndexedSkill::from_content(
            src,
            &d,
            &text,
            &files_of(&d),
            trees.get(&d).cloned(),
            Some(commit.sha.clone()),
            Some(info.default_branch.clone()),
            updated,
            repo_license.as_deref(),
            origin,
        ));
    }
    Ok(RepoFetch { skills: out, stars: info.stargazers_count, default_branch: info.default_branch, license: repo_license })
}

fn fetch_repo_git(ctx: &Ctx, src: &SourceId, filter: Option<&[String]>, origin: &str) -> Result<RepoFetch> {
    let m = open_mirror(ctx, src, Fetch::Always)?;
    let refs = crate::resolve::mirror_refs(&m)?;
    let branch = refs.default_branch.clone().or_else(|| refs.heads.keys().next().cloned()).context("no branches")?;
    let commit = refs.heads.get(&branch).cloned().context("no commit")?;
    let updated = m.commit_time(&commit);
    let root_license = ["LICENSE", "LICENSE.md", "LICENSE.txt"]
        .iter()
        .find_map(|f| m.read_file(&commit, f).ok())
        .map(|b| String::from_utf8_lossy(&b).to_string())
        .and_then(|t| crate::license::detect_text(&t).spdx);
    let mut out = Vec::new();
    for (d, tree) in m.skill_dirs(&commit)? {
        if !matches_filter(&d, filter) {
            continue;
        }
        let p = if d == "." { "SKILL.md".to_string() } else { format!("{d}/SKILL.md") };
        let Ok(bytes) = m.read_file(&commit, &p) else { continue };
        let spec = if d == "." { commit.clone() } else { format!("{commit}:{d}") };
        let listing = crate::git::git(&m.dir, &["ls-tree", "-r", &spec]).unwrap_or_default();
        let files: Vec<(String, bool)> = listing
            .lines()
            .filter_map(|l| {
                let (meta, path) = l.split_once('\t')?;
                Some((path.to_string(), meta.starts_with("100755")))
            })
            .collect();
        out.push(IndexedSkill::from_content(
            src,
            &d,
            &String::from_utf8_lossy(&bytes),
            &files,
            Some(tree),
            Some(commit.clone()),
            Some(branch.clone()),
            updated,
            root_license.as_deref(),
            origin,
        ));
    }
    Ok(RepoFetch { skills: out, stars: None, default_branch: branch, license: root_license })
}

/// Run `f` over items with bounded parallelism, preserving order.
pub fn parallel_map<T: Send, R: Send>(items: Vec<T>, workers: usize, f: impl Fn(T) -> R + Sync) -> Vec<R> {
    let n = items.len();
    let slots: Vec<std::sync::Mutex<Option<R>>> = (0..n).map(|_| std::sync::Mutex::new(None)).collect();
    let queue = std::sync::Mutex::new(items.into_iter().enumerate().collect::<Vec<_>>());
    std::thread::scope(|s| {
        for _ in 0..workers.max(1).min(n.max(1)) {
            s.spawn(|| {
                loop {
                    let next = queue.lock().unwrap().pop();
                    let Some((i, item)) = next else { break };
                    let r = f(item);
                    *slots[i].lock().unwrap() = Some(r);
                }
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().unwrap().unwrap()).collect()
}

// ---------------------------------------------------------------- marketplace adapter

#[derive(Debug, Deserialize)]
pub struct Marketplace {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub plugins: Vec<MarketplacePlugin>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct MarketplacePlugin {
    pub name: String,
    #[serde(default)]
    pub source: serde_json::Value,
    #[serde(default)]
    pub skills: Option<serde_json::Value>,
    #[serde(default)]
    pub category: Option<String>,
}

/// Read a file at a repository's default branch (API for GitHub, mirror otherwise).
fn read_repo_file(ctx: &Ctx, src: &SourceId, path: &str) -> Result<Option<Vec<u8>>> {
    if use_api(src) {
        let info: RepoInfo = ctx.gh.api_json(&src.host, &format!("/repos/{}", src.full_name()))?;
        return ctx.gh.raw_file(&src.host, src.full_name(), &info.default_branch, path);
    }
    let m = open_mirror(ctx, src, Fetch::IfStale)?;
    let refs = crate::resolve::mirror_refs(&m)?;
    let b = refs.default_branch.clone().context("no default branch")?;
    Ok(m.read_file(&b, path).ok())
}

/// Pointers (repo → path prefixes) for each plugin of a marketplace.
pub fn marketplace_pointers(market_src: &SourceId, mp: &Marketplace) -> Vec<(SourceId, Vec<String>, String, Option<String>)> {
    let plugin_root = mp
        .metadata
        .as_ref()
        .and_then(|m| m.get("pluginRoot"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim_start_matches("./").trim_matches('/').to_string());
    let mut out = Vec::new();
    for p in &mp.plugins {
        let (repo, root): (SourceId, String) = match &p.source {
            serde_json::Value::String(s) => {
                let rel = s.trim_start_matches("./").trim_matches('/').to_string();
                let rel = if !s.starts_with("./") && !s.starts_with('/') {
                    plugin_root.as_ref().map(|r| format!("{r}/{rel}")).unwrap_or(rel)
                } else {
                    rel
                };
                (market_src.clone(), if rel.is_empty() { ".".into() } else { rel })
            }
            serde_json::Value::Object(o) => {
                let kind = o.get("source").and_then(|v| v.as_str()).unwrap_or("");
                let path = o.get("path").and_then(|v| v.as_str()).unwrap_or(".").trim_start_matches("./").trim_matches('/').to_string();
                let path = if path.is_empty() { ".".to_string() } else { path };
                let repo = match kind {
                    "github" => o.get("repo").and_then(|v| v.as_str()).and_then(|r| SourceId::parse(r).ok()),
                    "url" | "git" | "git-subdir" => o.get("url").and_then(|v| v.as_str()).and_then(|u| parse_source_input(u).ok()),
                    _ => None,
                };
                match repo {
                    Some(r) => (r, path),
                    None => continue,
                }
            }
            _ => continue,
        };
        let prefixes: Vec<String> = match &p.skills {
            Some(serde_json::Value::Array(a)) => a
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| {
                    let s = s.trim_start_matches("./").trim_matches('/');
                    if root == "." { s.to_string() } else { format!("{root}/{s}") }
                })
                .collect(),
            Some(serde_json::Value::String(s)) => {
                let s = s.trim_start_matches("./").trim_matches('/');
                vec![if root == "." { s.to_string() } else { format!("{root}/{s}") }]
            }
            _ => vec![if root == "." { "skills".to_string() } else { format!("{root}/skills") }, root.clone()],
        };
        out.push((repo, prefixes, p.name.clone(), p.category.clone()));
    }
    out
}

pub fn index_marketplace(ctx: &Ctx, key: &str) -> Result<usize> {
    let src = parse_source_input(key)?;
    let bytes = read_repo_file(ctx, &src, ".claude-plugin/marketplace.json")?
        .ok_or_else(|| anyhow!("{src} has no .claude-plugin/marketplace.json"))?;
    let mp: Marketplace = serde_json::from_slice(&bytes).with_context(|| format!("parsing marketplace.json of {src}"))?;
    let catalog = format!("marketplace:{key}");
    let pointers = marketplace_pointers(&src, &mp);
    let mut by_repo: BTreeMap<SourceId, (Vec<String>, Vec<Option<String>>)> = BTreeMap::new();
    for (repo, prefixes, _name, category) in pointers {
        let e = by_repo.entry(repo).or_default();
        e.0.extend(prefixes);
        e.1.push(category);
    }
    let mut total = 0;
    let external = by_repo.keys().filter(|r| **r != src).count();
    if external > MARKETPLACE_REPO_CAP {
        ctx.ui.warn(&format!("{key}: {external} external plugin repositories; indexing the first {MARKETPLACE_REPO_CAP}"));
    }
    let mut ext_seen = 0;
    for (repo, (prefixes, cats)) in by_repo {
        if repo != src {
            ext_seen += 1;
            if ext_seen > MARKETPLACE_REPO_CAP {
                break;
            }
        }
        match index_repo(ctx, &repo, Some(&prefixes), key) {
            Ok(_) => {}
            Err(e) => {
                ctx.ui.warn(&format!("{key}: {repo}: {e:#}"));
                continue;
            }
        }
        let category = cats.into_iter().flatten().next();
        let mut st = ctx.state.conn.prepare("SELECT id, path FROM skills WHERE source=?1")?;
        let rows: Vec<(String, String)> = st.query_map([repo.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        for (id, path) in rows {
            if matches_filter(&path, Some(&prefixes)) {
                index::add_listing(ctx, &id, &catalog, None, category.as_deref())?;
                total += 1;
            }
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------- well-known adapter

pub fn wellknown_source(base: &str) -> SourceId {
    crate::wellknown::source_id(base)
}

pub fn index_wellknown(ctx: &Ctx, base: &str) -> Result<usize> {
    crate::wellknown::index(ctx, base)
}

// ---------------------------------------------------------------- pointer lists

/// Parse `apm.yml` / `skills-lock.json` into (repo, path prefix) pointers.
pub fn parse_pointer_file(name: &str, text: &str) -> Result<Vec<(SourceId, String)>> {
    let mut out = Vec::new();
    if name.ends_with(".json") {
        let v: serde_json::Value = serde_json::from_str(text)?;
        if let Some(skills) = v.get("skills").and_then(|s| s.as_object()) {
            for (_name, e) in skills {
                let Some(src) = e.get("source").and_then(|s| s.as_str()) else { continue };
                let Ok(repo) = parse_source_input(src) else { continue };
                let path = e
                    .get("skillPath")
                    .and_then(|s| s.as_str())
                    .map(|p| p.trim_end_matches("SKILL.md").trim_matches('/').to_string())
                    .unwrap_or_default();
                out.push((repo, path));
            }
        }
    } else {
        let v: serde_yaml::Value = serde_yaml::from_str(text)?;
        let deps = v.get("dependencies").and_then(|d| d.get("apm")).and_then(|a| a.as_sequence()).cloned().unwrap_or_default();
        for d in deps {
            let s = match &d {
                serde_yaml::Value::String(s) => s.clone(),
                serde_yaml::Value::Mapping(m) => {
                    match m.get("git").or_else(|| m.get("url")).or_else(|| m.get("package")).and_then(|x| x.as_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    }
                }
                _ => continue,
            };
            let s = s.split('#').next().unwrap_or(&s).split('@').next().unwrap_or(&s).to_string();
            if let Ok(Some(n)) = crate::id::normalize_url(&s) {
                if let crate::id::Normalized::Source(src, _) = n {
                    out.push((src, String::new()));
                }
                continue;
            }
            let segs: Vec<&str> = s.split('/').filter(|x| !x.is_empty()).collect();
            let (host, rest) =
                if segs.first().map(|f| f.contains('.')).unwrap_or(false) { (segs[0], &segs[1..]) } else { ("github.com", &segs[..]) };
            if rest.len() < 2 {
                continue;
            }
            let repo = SourceId::new(host, &format!("{}/{}", rest[0], rest[1]));
            out.push((repo, rest[2..].join("/")));
        }
    }
    Ok(out)
}

pub fn index_pointers(ctx: &Ctx, key: &str) -> Result<usize> {
    let path = ctx.paths.expand(key);
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let pointers = parse_pointer_file(&path.to_string_lossy(), &text)?;
    let catalog = format!("pointers:{key}");
    let mut by_repo: BTreeMap<SourceId, Vec<String>> = BTreeMap::new();
    for (repo, p) in pointers {
        by_repo.entry(repo).or_default().push(p);
    }
    let mut n = 0;
    for (repo, prefixes) in by_repo {
        if let Err(e) = index_repo(ctx, &repo, Some(&prefixes), key) {
            ctx.ui.warn(&format!("{key}: {repo}: {e:#}"));
            continue;
        }
        let mut st = ctx.state.conn.prepare("SELECT id, path FROM skills WHERE source=?1")?;
        let rows: Vec<(String, String)> = st.query_map([repo.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        for (id, p) in rows {
            if matches_filter(&p, Some(&prefixes)) {
                index::add_listing(ctx, &id, &catalog, None, None)?;
                n += 1;
            }
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------- live-query adapters

pub(crate) fn live_fresh(ctx: &Ctx, adapter: &str, q: &str) -> Result<bool> {
    let at: Option<i64> = ctx
        .state
        .conn
        .query_row("SELECT at FROM live_cache WHERE adapter=?1 AND query=?2", params![adapter, q], |r| r.get(0))
        .optional()?;
    Ok(at.map(|a| now() - a < LIVE_TTL_SECS).unwrap_or(false))
}

pub(crate) fn live_mark(ctx: &Ctx, adapter: &str, q: &str) -> Result<()> {
    ctx.state.conn.execute("INSERT OR REPLACE INTO live_cache(adapter, query, at) VALUES(?1,?2,?3)", params![adapter, q, now()])?;
    Ok(())
}

/// Ensure repositories are indexed recently enough (bounded, parallel work for live adapters).
pub(crate) fn ensure_repos_indexed(ctx: &Ctx, repos: &[SourceId]) {
    let interval = crate::user::load_manifest(ctx).map(|m| m.fetch_interval()).unwrap_or(std::time::Duration::from_secs(86_400));
    let stale: Vec<(SourceId, Option<Vec<String>>, String)> = repos
        .iter()
        .filter(|s| ctx.state.is_stale(&format!("index:{s}"), interval).unwrap_or(true))
        .map(|s| (s.clone(), None, s.to_string()))
        .collect();
    for (src, r) in index_repos(ctx, &stale) {
        match r {
            Ok(_) => {
                let _ = ctx.state.mark_fetched(&format!("index:{src}"));
            }
            Err(e) => ctx.ui.warn(&format!("{src}: {e:#}")),
        }
    }
}

/// Index only the pointed-to skill directories of each repository (catalog pointers
/// often land in large application repos). Freshness is tracked per directory.
pub(crate) fn ensure_pointers_indexed(ctx: &Ctx, pointers: &[(SourceId, String)]) {
    let interval = crate::user::load_manifest(ctx).map(|m| m.fetch_interval()).unwrap_or(std::time::Duration::from_secs(86_400));
    let mut by_repo: BTreeMap<SourceId, Vec<String>> = BTreeMap::new();
    for (src, dir) in pointers {
        let key = format!("index:{src}//{dir}");
        let repo_fresh = !ctx.state.is_stale(&format!("index:{src}"), interval).unwrap_or(true);
        if repo_fresh || !ctx.state.is_stale(&key, interval).unwrap_or(true) {
            continue;
        }
        let e = by_repo.entry(src.clone()).or_default();
        if !e.contains(dir) {
            e.push(dir.clone());
        }
    }
    let jobs: Vec<(SourceId, Option<Vec<String>>, String)> =
        by_repo.into_iter().map(|(s, d)| (s.clone(), Some(d), s.to_string())).collect();
    let dirs: BTreeMap<String, Vec<String>> = jobs.iter().map(|(s, d, _)| (s.to_string(), d.clone().unwrap_or_default())).collect();
    for (src, r) in index_repos(ctx, &jobs) {
        match r {
            Ok(_) => {
                for d in dirs.get(&src.to_string()).into_iter().flatten() {
                    let _ = ctx.state.mark_fetched(&format!("index:{src}//{d}"));
                }
            }
            Err(e) => ctx.ui.warn(&format!("{src}: {e:#}")),
        }
    }
}

#[derive(Deserialize)]
struct SkillsShResponse {
    #[serde(default)]
    skills: Vec<SkillsShItem>,
}

#[derive(Deserialize)]
struct SkillsShItem {
    #[serde(default, rename = "skillId")]
    skill_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    installs: Option<i64>,
    #[serde(default)]
    source: Option<String>,
}

/// skills.sh live search (unauthenticated endpoint used by `npx skills find`).
pub fn live_skills_sh(ctx: &Ctx, q: &str, max_repos: usize) -> Result<usize> {
    if ctx.opts.offline || q.trim().is_empty() || live_fresh(ctx, "skills.sh", q)? {
        return Ok(0);
    }
    let base = std::env::var("TRICKS_SKILLS_SH_URL").unwrap_or_else(|_| "https://skills.sh".into());
    let url = format!("{base}/api/search?q={}&limit=20", encode_query(q));
    let r = ctx.gh.get_public(&url)?;
    if r.status != 200 {
        bail!("skills.sh search returned {}", r.status);
    }
    let resp: SkillsShResponse = serde_json::from_slice(&r.body)?;
    let mut repos: Vec<SourceId> = Vec::new();
    for it in &resp.skills {
        if let Some(src) = it.source.as_deref().and_then(|s| SourceId::parse(s).ok())
            && !repos.contains(&src)
        {
            repos.push(src);
        }
    }
    repos.truncate(max_repos);
    ensure_repos_indexed(ctx, &repos);
    let mut n = 0;
    for it in resp.skills {
        let Some(src) = it.source.as_deref().and_then(|s| SourceId::parse(s).ok()) else { continue };
        let key = it.skill_id.or(it.name).unwrap_or_default();
        if let Some(id) = index::find_in_source(ctx, &src.to_string(), &key)? {
            index::add_listing(ctx, &id, "skills.sh", it.installs, None)?;
            n += 1;
        }
    }
    live_mark(ctx, "skills.sh", q)?;
    Ok(n)
}

#[derive(Deserialize)]
struct CodeSearch {
    #[serde(default)]
    items: Vec<CodeItem>,
}

#[derive(Deserialize)]
struct CodeItem {
    path: String,
    repository: CodeRepo,
}

#[derive(Deserialize)]
struct CodeRepo {
    full_name: String,
}

/// GitHub code search for SKILL.md files (requires a token).
pub fn live_github_search(ctx: &Ctx, q: &str, max_repos: usize) -> Result<usize> {
    if ctx.opts.offline || q.trim().is_empty() || ctx.gh.token("github.com").is_none() || live_fresh(ctx, "github", q)? {
        return Ok(0);
    }
    let path = format!("/search/code?q={}+filename:SKILL.md&per_page=30", encode_query(q));
    let res: CodeSearch = match ctx.gh.api_json("github.com", &path) {
        Ok(r) => r,
        Err(e) if is_not_found(&e) => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut by_repo: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for it in res.items {
        let dir = it.path.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_else(|| ".".into());
        by_repo.entry(it.repository.full_name).or_default().push(dir);
    }
    let mut n = 0;
    let jobs: Vec<(SourceId, Option<Vec<String>>, String)> = by_repo
        .into_iter()
        .take(max_repos)
        .filter_map(|(repo, dirs)| {
            SourceId::parse(&repo).ok().map(|s| {
                let o = s.to_string();
                (s, Some(dirs), o)
            })
        })
        .collect();
    for (src, r) in index_repos(ctx, &jobs) {
        match r {
            Ok(k) => n += k,
            Err(e) => ctx.ui.warn(&format!("github search: {src}: {e:#}")),
        }
    }
    live_mark(ctx, "github", q)?;
    Ok(n)
}

// ---------------------------------------------------------------- identity

/// Cached (login, orgs) for trust facets and the `auto` policy check.
pub fn cached_identity(ctx: &Ctx, host: &str) -> Option<(String, Vec<String>)> {
    if let Ok(v) = std::env::var("TRICKS_IDENTITY") {
        let mut p = v.split(',').map(|s| s.trim().to_string());
        let login = p.next().unwrap_or_default();
        return Some((login, p.collect()));
    }
    let row: Option<(String, String, i64)> = ctx
        .state
        .conn
        .query_row("SELECT login, orgs, fetched_at FROM identity WHERE host=?1", [host], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()
        .ok()
        .flatten();
    if let Some((login, orgs, at)) = &row
        && (now() - at < 86_400 || ctx.opts.offline)
    {
        return Some((login.clone(), orgs.split(',').filter(|s| !s.is_empty()).map(String::from).collect()));
    }
    if ctx.opts.offline || ctx.gh.token(host).is_none() {
        return row.map(|(l, o, _)| (l, o.split(',').filter(|s| !s.is_empty()).map(String::from).collect()));
    }
    match ctx.gh.identity(host) {
        Ok((login, orgs)) => {
            let _ = ctx.state.conn.execute(
                "INSERT OR REPLACE INTO identity(host, login, orgs, fetched_at) VALUES(?1,?2,?3,?4)",
                params![host, login, orgs.join(","), now()],
            );
            Some((login, orgs))
        }
        Err(_) => None,
    }
}

/// Repositories the signed-in user has starred (`owner/repo`, lowercase), for the
/// "starred by you" trust facet. Refreshed daily; `TRICKS_STARRED` overrides (tests).
pub fn cached_starred(ctx: &Ctx, host: &str) -> std::collections::HashSet<String> {
    if let Ok(v) = std::env::var("TRICKS_STARRED") {
        return v.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    }
    let key = format!("starred_at:{host}");
    let fresh = ctx.state.meta_get(&key).ok().flatten().and_then(|t| t.parse::<i64>().ok()).map(|t| now() - t < 86_400).unwrap_or(false);
    if !fresh && !ctx.opts.offline && ctx.gh.token(host).is_some() {
        #[derive(Deserialize)]
        struct Star {
            full_name: String,
        }
        let mut all: Vec<String> = Vec::new();
        let mut ok = true;
        for page in 1..=10 {
            match ctx.gh.api_json::<Vec<Star>>(host, &format!("/user/starred?per_page=100&page={page}")) {
                Ok(v) => {
                    let n = v.len();
                    all.extend(v.into_iter().map(|s| s.full_name.to_ascii_lowercase()));
                    if n < 100 {
                        break;
                    }
                }
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            let _ = ctx.state.conn.execute("DELETE FROM starred WHERE host=?1", [host]);
            for r in &all {
                let _ = ctx.state.conn.execute("INSERT OR IGNORE INTO starred(host, repo) VALUES(?1, ?2)", params![host, r]);
            }
            let _ = ctx.state.meta_set(&key, &now().to_string());
        }
    }
    let mut out = std::collections::HashSet::new();
    if let Ok(mut st) = ctx.state.conn.prepare("SELECT repo FROM starred WHERE host=?1")
        && let Ok(rows) = st.query_map([host], |r| r.get::<_, String>(0))
    {
        out.extend(rows.flatten());
    }
    out
}

pub fn api_base_for(host: &str) -> String {
    GitHub::api_base(host)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marketplace_pointer_forms() {
        let json = r#"{
          "name": "m", "owner": {"name": "x"},
          "plugins": [
            {"name": "all", "source": "./", "strict": false, "skills": ["./skills/pdf", "./skills/docx"]},
            {"name": "ext", "source": {"source": "github", "repo": "acme/tools"}},
            {"name": "sub", "source": {"source": "git-subdir", "url": "https://github.com/acme/mono.git", "path": "plugins/a"}},
            {"name": "rel", "source": "./plugins/rel"}
          ]
        }"#;
        let mp: Marketplace = serde_json::from_str(json).unwrap();
        let src = SourceId::new("github.com", "acme/market");
        let p = marketplace_pointers(&src, &mp);
        assert_eq!(p[0].0, src);
        assert_eq!(p[0].1, vec!["skills/pdf", "skills/docx"]);
        assert_eq!(p[1].0.to_string(), "github.com/acme/tools");
        assert_eq!(p[1].1, vec!["skills", "."]);
        assert_eq!(p[2].1, vec!["plugins/a/skills", "plugins/a"]);
        assert_eq!(p[3].1, vec!["plugins/rel/skills", "plugins/rel"]);
    }

    #[test]
    fn pointer_files() {
        let apm =
            "name: x\ndependencies:\n  apm:\n    - anthropics/skills/skills/frontend-design\n    - microsoft/apm-sample-package#v1.0.0\n";
        let p = parse_pointer_file("apm.yml", apm).unwrap();
        assert_eq!(p[0].0.to_string(), "github.com/anthropics/skills");
        assert_eq!(p[0].1, "skills/frontend-design");
        assert_eq!(p[1].1, "");
        let lock = r#"{"version":1,"skills":{"pdf":{"source":"anthropics/skills","sourceType":"github","skillPath":"skills/pdf/SKILL.md","computedHash":"x"}}}"#;
        let p = parse_pointer_file("skills-lock.json", lock).unwrap();
        assert_eq!(p[0].1, "skills/pdf");
    }

    #[test]
    fn filters() {
        let f = vec!["skills".to_string()];
        assert!(matches_filter("skills/pdf", Some(&f)));
        assert!(!matches_filter("other/pdf", Some(&f)));
        assert!(matches_filter("x", None));
    }
}
