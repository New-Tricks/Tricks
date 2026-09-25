//! SourceRepo authoring (spec §10): vendoring with a recorded base, upstream merges left
//! uncommitted for review, branch experiments via worktrees, and dev deployments.

use crate::agents::{self, Agent};
use crate::config::{self, Policy, RepoLocked, RepoSkill, SourceRepoLock, SourceRepoManifest, WorkFile};
use crate::ctx::Ctx;
use crate::deploy::{self, PlaceRequest, Scope};
use crate::git::{self, Mirror, git};
use crate::id::{SkillId, SkillSpec, valid_skill_name};
use crate::links::LinkTarget;
use crate::merge::{self, Conflict, MergeOutcome};
use crate::resolve::{Fetch, resolve_skill};
use crate::risk::RiskReport;
use crate::skill::SkillDoc;
use crate::state::now;
use crate::store;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct SourceRepo {
    pub root: PathBuf,
    pub name: String,
    #[serde(skip)]
    pub manifest: SourceRepoManifest,
    #[serde(skip)]
    pub lock: SourceRepoLock,
}

impl SourceRepo {
    pub fn open(root: &Path) -> Result<SourceRepo> {
        let root = crate::paths::canon(root).unwrap_or(root.to_path_buf());
        let manifest = SourceRepoManifest::load(&root)?;
        let lock = SourceRepoLock::load(&root)?;
        let name = manifest
            .source_repo
            .name
            .clone()
            .unwrap_or_else(|| root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "skills".into()));
        Ok(SourceRepo { root, name, manifest, lock })
    }

    pub fn reload(&mut self) -> Result<()> {
        *self = SourceRepo::open(&self.root)?;
        Ok(())
    }

    pub fn skill(&self, name: &str) -> Result<&RepoSkill> {
        self.manifest.skills.get(name).with_context(|| {
            format!(
                "no skill `{name}` in source repo {} (have: {})",
                self.name,
                self.manifest.skills.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
    }

    pub fn skill_dir(&self, name: &str) -> Result<PathBuf> {
        Ok(self.root.join(&self.skill(name)?.path))
    }

    pub fn key(&self) -> String {
        crate::paths::path_key(&self.root)
    }

    pub fn skill_key(&self, name: &str) -> String {
        format!("ws:{}//{name}", self.root.display())
    }

    pub fn agents(&self, ctx: &Ctx) -> Result<Vec<&'static Agent>> {
        if !self.manifest.source_repo.agents.is_empty() {
            return agents::parse_list(&self.manifest.source_repo.agents);
        }
        crate::user::default_agents(&crate::user::config(ctx)?)
    }

    fn edit_doc<F: FnOnce(&mut toml_edit::DocumentMut) -> Result<()>>(&self, f: F) -> Result<()> {
        let path = self.root.join(config::REPO_MANIFEST);
        let mut doc = config::load_doc(&path)?;
        f(&mut doc)?;
        config::save_doc(&path, &doc)
    }

    pub fn set_skill_field(&self, name: &str, key: &str, value: Option<&str>) -> Result<()> {
        self.edit_doc(|doc| {
            let t = config::table_mut(doc, &["skills", name]);
            match value {
                Some(v) => {
                    t.insert(key, config::str_value(v));
                }
                None => {
                    t.remove(key);
                }
            }
            Ok(())
        })
    }
}

pub fn current(ctx: &Ctx) -> Result<Option<SourceRepo>> {
    match config::find_source_repo(&ctx.opts.cwd) {
        Some(root) => Ok(Some(SourceRepo::open(&root)?)),
        None => Ok(None),
    }
}

pub fn require(ctx: &Ctx) -> Result<SourceRepo> {
    current(ctx)?.context("not inside a New Tricks source repo (run `tricks init` in a git repository)")
}

/// Registered source repos (user config) plus the current one.
pub fn all_source_repos(ctx: &Ctx) -> Result<Vec<SourceRepo>> {
    let m = crate::user::config(ctx)?;
    let mut roots: BTreeSet<PathBuf> = m.source_repos.values().map(|p| ctx.paths.expand(p)).collect();
    if let Some(r) = config::find_source_repo(&ctx.opts.cwd) {
        roots.insert(r);
    }
    Ok(roots.into_iter().filter(|r| r.join(config::REPO_MANIFEST).is_file()).filter_map(|r| SourceRepo::open(&r).ok()).collect())
}

pub fn vendored_upstreams(ctx: &Ctx) -> Result<BTreeSet<String>> {
    let mut s = BTreeSet::new();
    for ws in all_source_repos(ctx)? {
        for sk in ws.manifest.skills.values() {
            if let Some(u) = &sk.upstream {
                s.insert(u.clone());
            }
        }
    }
    Ok(s)
}

/// Placement key for a directory inside a source repo skill.
pub fn skill_key_for_dir(dir: &Path) -> Option<String> {
    let root = config::find_source_repo(dir)?;
    let ws = SourceRepo::open(&root).ok()?;
    let rel = dir.strip_prefix(&ws.root).ok()?.to_string_lossy().replace('\\', "/");
    ws.manifest.skills.iter().find(|(_, s)| s.path.trim_end_matches('/') == rel).map(|(n, _)| ws.skill_key(n))
}

// ---------------------------------------------------------------- init

const MANIFEST_TEMPLATE: &str = r#"# New Tricks source repo: https://github.com/new-tricks/tricks
# Skills authored or customized here. Vendored skills record their upstream; the base
# commit lives in tricks.lock.

[source-repo]
# agents = ["claude", "codex"]   # agents for dev deployments (default: user setting)

[skills]

[lint]
ignore = []

# [publish.targets.public]
# repo    = "acme/my-skills-public"    # distribution repository: owner/repo, a git URL or a path
# skills  = ["*"]
# exclude = ["evals/**", "notes/**", "*.draft.md"]
"#;

#[derive(Debug, Serialize)]
pub struct InitReport {
    pub root: String,
    pub name: String,
    pub created: bool,
    pub agent_skill: Vec<String>,
}

pub fn init(ctx: &Ctx, name: Option<&str>, agent_skill: bool) -> Result<InitReport> {
    let cwd = ctx.opts.cwd.clone();
    let root = match git::repo_root(&cwd) {
        Some(r) => r,
        None => {
            git(&cwd, &["init", "-q"])?;
            ctx.ui.info(&format!("initialized a git repository in {}", cwd.display()));
            cwd.clone()
        }
    };
    let root = crate::paths::canon(&root)?;
    let manifest = root.join(config::REPO_MANIFEST);
    let created = !manifest.exists();
    if created {
        let mut text = MANIFEST_TEMPLATE.to_string();
        if let Some(n) = name {
            text = text.replace("[source-repo]\n", &format!("[source-repo]\nname = \"{n}\"\n"));
        }
        std::fs::write(&manifest, text)?;
        SourceRepoLock { version: 1, ..Default::default() }.save(&root)?;
        std::fs::create_dir_all(root.join("skills"))?;
        ensure_ignored(&root)?;
    }
    let ws = SourceRepo::open(&root)?;
    // Register in the user config.
    let path = ctx.paths.user_config();
    let mut doc = config::load_doc(&path)?;
    let m = crate::user::config(ctx)?;
    let contracted = ctx.paths.contract(&root);
    if !m.source_repos.values().any(|p| ctx.paths.expand(p) == root) {
        let mut key = ws.name.clone();
        let mut i = 2;
        while m.source_repos.contains_key(&key) {
            key = format!("{}-{i}", ws.name);
            i += 1;
        }
        config::table_mut(&mut doc, &["source-repos"]).insert(&key, config::str_value(&contracted));
        config::save_doc(&path, &doc)?;
    }
    let mut placed = Vec::new();
    if agent_skill {
        placed = crate::agentskill::install(ctx, &ws.agents(ctx)?, &Scope::Project(root.clone()))?;
    }
    Ok(InitReport { root: root.to_string_lossy().to_string(), name: ws.name, created, agent_skill: placed })
}

// ---------------------------------------------------------------- vendor / new

#[derive(Debug, Serialize)]
pub struct VendorReport {
    pub name: String,
    pub path: String,
    pub upstream: Option<String>,
    pub base: Option<String>,
    pub license: Option<config::LicenseRecord>,
    pub risk: Vec<String>,
}

fn track_for(spec: &SkillSpec, kind: &str, name: &str) -> String {
    match (&spec.reference, kind) {
        (Some(_), "branch") => name.to_string(),
        _ => "latest".to_string(),
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct VendorOptions<'a> {
    pub name: Option<&'a str>,
    pub path: Option<&'a str>,
    /// Take the files from this local copy of the upstream skill (made earlier)...
    pub from: Option<&'a str>,
    /// ...which started from this upstream revision (commit, tag, or catalog version).
    pub base: Option<&'a str>,
}

/// Where a new skill goes in the source repo (validates the name and the path).
fn destination(ws: &SourceRepo, name: &str, path: Option<&str>) -> Result<(String, PathBuf)> {
    if !valid_skill_name(name) {
        bail!("`{name}` is not a valid skill name (lowercase letters, digits and single hyphens); pass --name");
    }
    if ws.manifest.skills.contains_key(name) {
        bail!("source repo already has a skill named `{name}`");
    }
    let rel = path.map(|p| p.trim_matches('/').to_string()).unwrap_or_else(|| format!("skills/{name}"));
    let dest = ws.root.join(&rel);
    if dest.exists() {
        bail!("{} already exists", dest.display());
    }
    Ok((rel, dest))
}

/// Record a vendored skill in the manifest and lock.
fn record(ws: &SourceRepo, name: &str, rel: &str, upstream: Option<(&str, &str)>, locked: RepoLocked) -> Result<()> {
    ws.edit_doc(|doc| {
        let t = config::table_mut(doc, &["skills", name]);
        t.set_implicit(false);
        t.insert("path", config::str_value(rel));
        if let Some((u, track)) = upstream {
            t.insert("upstream", config::str_value(u));
            t.insert("track", config::str_value(track));
            t.insert("update", config::str_value("review"));
        }
        Ok(())
    })?;
    let mut lock = ws.lock.clone();
    lock.skills.insert(name.to_string(), locked);
    lock.save(&ws.root)
}

fn confirm_licence(ctx: &Ctx, what: &str, lic: &config::LicenseRecord) -> Result<()> {
    let class = crate::license::Class::parse(&lic.class);
    if class == crate::license::Class::Block || class == crate::license::Class::NonCommercial {
        let details = vec![
            format!("licence: {} ({}, via {})", lic.spdx.as_deref().unwrap_or("none"), lic.class, lic.source),
            "its terms may prohibit modification or redistribution; you are responsible for complying".to_string(),
        ];
        if !ctx.confirm(&format!("Vendor {what} anyway?"), &details)? {
            bail!("cancelled");
        }
    }
    Ok(())
}

/// Bring an upstream skill (git or catalog-hosted) into the source repo to customize it;
/// the upstream and base are recorded for `update`.
pub fn vendor(ctx: &Ctx, ws: &SourceRepo, input: &str, o: &VendorOptions) -> Result<VendorReport> {
    if !input.contains("//") && local_skill_dir(ctx, input).is_some() {
        bail!(
            "`{input}` is a local folder: add it with `tricks create <name> --from {input}` (or `tricks vendor <upstream> --from {input} --base <rev>` if it is a copy of an upstream skill)"
        );
    }
    if o.from.is_some() != o.base.is_some() {
        bail!("--from and --base go together: the local copy and the upstream revision it started from");
    }
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    let hosted_id = SkillId::new(spec.source.clone(), &spec.selector);
    if crate::hosted::kind(&hosted_id).is_some() {
        let version = o.base.or(spec.reference.as_deref().filter(|r| *r != "latest"));
        return vendor_hosted(ctx, ws, &hosted_id, version, o);
    }
    let spec = match o.base {
        Some(b) => SkillSpec { reference: Some(b.to_string()), ..spec },
        None => spec,
    };
    let r = resolve_skill(ctx, &spec, Fetch::IfStale)?;
    let name = o.name.map(String::from).unwrap_or_else(|| crate::user::placement_name(&r.name, &r.id));
    let (rel, dest) = destination(ws, &name, o.path)?;
    let store_dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    let lic = crate::inspect::detect_license_in_dir(ctx, &r, &store_dir);
    confirm_licence(ctx, &r.canonical(), &lic)?;
    let src = match o.from {
        Some(f) => local_skill_dir(ctx, f).with_context(|| format!("{f} has no SKILL.md"))?,
        None => store_dir,
    };
    store::copy_dir(&src, &dest)?;
    let upstream = r.id.to_string();
    // A copy vendored at a given base tracks the latest upstream from there.
    let track = if o.base.is_some() { "latest".to_string() } else { track_for(&spec, &r.reference.kind, &r.reference.name) };
    let locked = RepoLocked {
        base: Some(r.reference.commit.clone()),
        base_tree: Some(r.tree.clone()),
        upstream_path: None,
        license: Some(lic.clone()),
    };
    record(ws, &name, &rel, Some((&upstream, &track)), locked)?;
    Ok(VendorReport {
        name,
        path: rel,
        upstream: Some(upstream),
        base: Some(r.reference.commit),
        license: Some(lic),
        risk: RiskReport::scan_dir(&dest).summary(),
    })
}

fn vendor_hosted(ctx: &Ctx, ws: &SourceRepo, id: &SkillId, version: Option<&str>, o: &VendorOptions) -> Result<VendorReport> {
    if o.base.is_some() && crate::hosted::kind(id) == Some(crate::hosted::Kind::WellKnown) {
        bail!("{id}: .well-known catalogs only serve the latest revision, so an earlier base cannot be recorded; vendor it without --base");
    }
    let st = match crate::hosted::fetch_to_store(ctx, id, version)? {
        crate::hosted::StoreOutcome::Stored(st) => st,
        crate::hosted::StoreOutcome::Redirect(git_id) => {
            ctx.ui.info(&format!("{id} is hosted on GitHub; vendoring {git_id}"));
            return vendor(ctx, ws, &git_id, o);
        }
    };
    let doc = SkillDoc::parse(&std::fs::read_to_string(st.dir.join("SKILL.md")).unwrap_or_default());
    let name = o.name.map(String::from).unwrap_or_else(|| crate::user::placement_name(&doc.name.unwrap_or_default(), id));
    let (rel, dest) = destination(ws, &name, o.path)?;
    let lic = crate::hosted::license(id, &st.dir);
    confirm_licence(ctx, &format!("{id}@{}", st.label), &lic)?;
    let src = match o.from {
        Some(f) => local_skill_dir(ctx, f).with_context(|| format!("{f} has no SKILL.md"))?,
        None => st.dir.clone(),
    };
    store::copy_dir(&src, &dest)?;
    let upstream = id.to_string();
    let locked =
        RepoLocked { base: Some(st.commit.clone()), base_tree: Some(st.tree.clone()), upstream_path: None, license: Some(lic.clone()) };
    record(ws, &name, &rel, Some((&upstream, "latest")), locked)?;
    Ok(VendorReport {
        name,
        path: rel,
        upstream: Some(upstream),
        base: Some(st.commit),
        license: Some(lic),
        risk: RiskReport::scan_dir(&dest).summary(),
    })
}

fn local_skill_dir(ctx: &Ctx, input: &str) -> Option<PathBuf> {
    let p = ctx.paths.expand(input);
    let p = if p.is_absolute() { p } else { ctx.opts.cwd.join(p) };
    p.join("SKILL.md").is_file().then(|| crate::paths::canon(&p).unwrap_or(p))
}

/// `tricks create <name> [--from <folder>]`: a new local original, scaffolded or taken
/// from an existing skill folder.
pub fn create(ctx: &Ctx, ws: &SourceRepo, name: &str, description: Option<&str>, from: Option<&str>) -> Result<VendorReport> {
    let Some(f) = from else { return scaffold(ws, name, description) };
    if description.is_some() {
        bail!("--description is for new skills; a skill created --from a folder keeps its own");
    }
    let src = local_skill_dir(ctx, f).with_context(|| format!("{f} has no SKILL.md"))?;
    let (rel, dest) = destination(ws, name, None)?;
    store::copy_dir(&src, &dest)?;
    let lic = crate::license::detect(&crate::inspect::gather_dir(&dest));
    record(ws, name, &rel, None, RepoLocked { license: Some(lic.clone()), ..Default::default() })?;
    Ok(VendorReport {
        name: name.into(),
        path: rel,
        upstream: None,
        base: None,
        license: Some(lic),
        risk: RiskReport::scan_dir(&dest).summary(),
    })
}

#[derive(Debug, Serialize)]
pub struct RemoveReport {
    pub name: String,
    pub path: String,
    pub unlinked: Vec<String>,
}

/// `tricks remove <skill>`: unlink it, delete its folder, and drop it from the manifest
/// and lock (left uncommitted, like every other change to the repo).
pub fn remove(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<RemoveReport> {
    let rel = ws.skill(name)?.path.clone();
    if merge_state(ctx, ws, name)?.is_some() {
        bail!("an upstream update of `{name}` is in progress; finish it with `tricks update --continue` or `--abort` first");
    }
    let dirty = git(&ws.root, &["status", "--porcelain", "--", &rel]).map(|o| !o.trim().is_empty()).unwrap_or(false);
    if dirty && !ctx.confirm(&format!("`{name}` has uncommitted changes that will be lost. Remove it anyway?"), &[])? {
        bail!("cancelled");
    }
    let mut unlinked = Vec::new();
    for p in ctx.state.placements("WHERE skill=?1", &[&ws.skill_key(name)])? {
        deploy::remove_placement(ctx, &p)?;
        unlinked.push(p.path);
    }
    ctx.state.conn.execute(
        "DELETE FROM meta WHERE key IN (?1, ?2, ?3)",
        params![editing_key(ws, name), snapshot_key(ws, name), hosted_up_key(ws, name)],
    )?;
    let dir = ws.root.join(&rel);
    if dir.exists() {
        deploy::remove_path(&dir)?;
    }
    ws.edit_doc(|doc| {
        if let Some(t) = doc.get_mut("skills").and_then(|t| t.as_table_like_mut()) {
            t.remove(name);
        }
        Ok(())
    })?;
    let mut lock = ws.lock.clone();
    lock.skills.remove(name);
    lock.save(&ws.root)?;
    let _ = store::gc(ctx, false);
    Ok(RemoveReport { name: name.into(), path: rel, unlinked })
}

fn scaffold(ws: &SourceRepo, name: &str, description: Option<&str>) -> Result<VendorReport> {
    if !valid_skill_name(name) {
        bail!("`{name}` is not a valid skill name (lowercase letters, digits and single hyphens; max 64)");
    }
    if ws.manifest.skills.contains_key(name) {
        bail!("source repo already has a skill named `{name}`");
    }
    let rel = format!("skills/{name}");
    let dest = ws.root.join(&rel);
    if dest.exists() {
        bail!("{} already exists", dest.display());
    }
    std::fs::create_dir_all(&dest)?;
    let desc = description.unwrap_or("TODO: what this skill does. Use when … (describe the situations that should trigger it).");
    let title: String = name
        .split('-')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ");
    std::fs::write(
        dest.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {desc}\n---\n\n# {title}\n\n## Instructions\n\n1. \n\n## Examples\n\n"),
    )?;
    ws.edit_doc(|doc| {
        let t = config::table_mut(doc, &["skills", name]);
        t.set_implicit(false);
        t.insert("path", config::str_value(&rel));
        Ok(())
    })?;
    Ok(VendorReport { name: name.into(), path: rel, upstream: None, base: None, license: None, risk: vec![] })
}

// ---------------------------------------------------------------- upstream update

#[derive(Debug, Serialize)]
pub struct UpdateItem {
    pub name: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub to_ref: Option<String>,
    /// update-available | up-to-date | merged | conflicts | continued | aborted | skipped | error
    pub state: String,
    pub outcome: Option<MergeOutcome>,
    pub risk: Vec<String>,
    pub incoming: Vec<String>,
    pub message: Option<String>,
}

impl UpdateItem {
    fn new(name: &str, state: &str) -> UpdateItem {
        UpdateItem {
            name: name.into(),
            from: None,
            to: None,
            to_ref: None,
            state: state.into(),
            outcome: None,
            risk: vec![],
            incoming: vec![],
            message: None,
        }
    }
}

#[derive(Debug, Serialize, Default)]
pub struct UpdateReport {
    pub items: Vec<UpdateItem>,
}

fn upstream_of(ws: &SourceRepo, name: &str) -> Result<Option<(SkillId, String)>> {
    let s = ws.skill(name)?;
    let Some(u) = &s.upstream else { return Ok(None) };
    let mut id = SkillId::parse_canonical(u)?;
    if let Some(p) = ws.lock.skills.get(name).and_then(|l| l.upstream_path.clone()) {
        id.path = p;
    }
    let track = s.track.clone().unwrap_or_else(|| "latest".into());
    let req = if track == "latest" {
        "latest".to_string()
    } else if crate::resolve::parse_semver(&track).is_some() {
        track
    } else {
        format!("refs/heads/{track}")
    };
    Ok(Some((id, req)))
}

/// B (the recorded base) and U (the current upstream) of a vendored skill, as
/// directories, for git and catalog-hosted upstreams alike.
pub struct Sides {
    pub base: String,
    pub base_tree: String,
    pub base_dir: PathBuf,
    pub up_commit: String,
    pub up_tree: String,
    pub up_dir: PathBuf,
    /// Branch, tag or version the upstream resolved to.
    pub up_label: String,
    /// New upstream path when the skill moved upstream (git only).
    pub renamed: Option<String>,
    pub incoming: Vec<String>,
}

impl Sides {
    pub fn up_to_date(&self) -> bool {
        self.up_commit == self.base || self.up_tree == self.base_tree
    }
}

/// Resolve B and U for `name` (`None` for local originals). `Fetch::Never` works from
/// cached mirrors and snapshots only.
pub fn sides(ctx: &Ctx, ws: &SourceRepo, name: &str, fetch: Fetch) -> Result<Option<Sides>> {
    let Some((mut id, req)) = upstream_of(ws, name)? else { return Ok(None) };
    let locked = ws.lock.skills.get(name).cloned().unwrap_or_default();
    let base = locked.base.clone().context("no base recorded in tricks.lock; re-vendor, or vendor the folder with --upstream/--base")?;
    if crate::hosted::kind(&id).is_some() {
        return hosted_sides(ctx, ws, name, &id, &locked, base, fetch).map(Some);
    }
    let mirror = crate::resolve::open_mirror(ctx, &id.source, fetch)?;
    let refs = crate::resolve::mirror_refs(&mirror)?;
    let mut warns = Vec::new();
    let u_ref = crate::resolve::resolve_ref(&mirror, &refs, Some(&req), &mut warns)?;
    if fetch != Fetch::Never {
        mirror.ensure_commit(&base)?;
    }
    let base_path = id.path.clone();
    let mut renamed = None;
    let up_tree = match mirror.tree_at(&u_ref.commit, &id.path) {
        Some(t) => t,
        None => match mirror.renamed_path(&base, &u_ref.commit, &id.path) {
            Some(newp) => {
                let t = mirror.tree_at(&u_ref.commit, &newp).context("renamed path has no tree")?;
                renamed = Some(newp.clone());
                id.path = newp;
                t
            }
            None => bail!("upstream no longer contains {id} (deleted or moved); the vendored copy is kept"),
        },
    };
    let base_tree = mirror.tree_at(&base, &base_path).context("base revision missing the skill directory")?;
    let base_dir = store::from_mirror(ctx, &mirror, &base, &base_path, &base_tree)?;
    let up_dir = store::from_mirror(ctx, &mirror, &u_ref.commit, &id.path, &up_tree)?;
    let incoming = file_changes(&mirror.dir, &base, &u_ref.commit, &id.path);
    Ok(Some(Sides { base, base_tree, base_dir, up_commit: u_ref.commit, up_tree, up_dir, up_label: u_ref.name, renamed, incoming }))
}

fn hosted_up_key(ws: &SourceRepo, name: &str) -> String {
    format!("hosted-upstream:{}:{name}", ws.root.display())
}

fn hosted_sides(ctx: &Ctx, ws: &SourceRepo, name: &str, id: &SkillId, locked: &RepoLocked, base: String, fetch: Fetch) -> Result<Sides> {
    let base_tree = locked.base_tree.clone().context("no base snapshot recorded in tricks.lock; re-vendor")?;
    let cached = store::entry_path(ctx, &base_tree);
    let base_dir = if cached.is_dir() {
        cached
    } else {
        match if fetch == Fetch::Never { None } else { crate::hosted::refetch(ctx, id, &base)? } {
            Some(s) if s.tree == base_tree => s.dir,
            Some(_) => bail!("{id} {} no longer matches the recorded base", crate::hosted::label(&base)),
            None => bail!(
                "the base snapshot of `{name}` ({}) is no longer cached and {id} cannot serve it again; re-vendor to reset the base",
                crate::hosted::label(&base)
            ),
        }
    };
    let key = hosted_up_key(ws, name);
    let (up_commit, up_tree, up_dir) = if fetch == Fetch::Never {
        let last = ctx.state.meta_get(&key)?.and_then(|v| v.split_once(' ').map(|(c, t)| (c.to_string(), t.to_string())));
        match last {
            Some((c, t)) if store::entry_path(ctx, &t).is_dir() => {
                let d = store::entry_path(ctx, &t);
                (c, t, d)
            }
            _ => (base.clone(), base_tree.clone(), base_dir.clone()),
        }
    } else {
        crate::hosted::refresh_listing(ctx, id)?;
        match crate::hosted::fetch_to_store(ctx, id, None)? {
            crate::hosted::StoreOutcome::Stored(s) => {
                ctx.state.meta_set(&key, &format!("{} {}", s.commit, s.tree))?;
                (s.commit, s.tree, s.dir)
            }
            crate::hosted::StoreOutcome::Redirect(g) => bail!("{id} is now served from GitHub ({g}); vendor it from there instead"),
        }
    };
    let incoming = dir_changes(&base_dir, &up_dir);
    let up_label = crate::hosted::label(&up_commit);
    Ok(Sides { base, base_tree, base_dir, up_commit, up_tree, up_dir, up_label, renamed: None, incoming })
}

/// Store trees the source repos need kept: base and last-fetched upstream snapshots of
/// skills vendored from catalog-hosted upstreams.
pub fn hosted_snapshots(ctx: &Ctx) -> Result<BTreeSet<String>> {
    let mut keep = BTreeSet::new();
    for ws in all_source_repos(ctx)? {
        for (name, s) in &ws.manifest.skills {
            let hosted =
                s.upstream.as_deref().and_then(|u| SkillId::parse_canonical(u).ok()).is_some_and(|id| crate::hosted::kind(&id).is_some());
            if hosted && let Some(t) = ws.lock.skills.get(name).and_then(|l| l.base_tree.clone()) {
                keep.insert(t);
            }
        }
    }
    let mut st = ctx.state.conn.prepare("SELECT value FROM meta WHERE key LIKE 'hosted-upstream:%'")?;
    for v in st.query_map([], |r| r.get::<_, String>(0))? {
        if let Some((_, t)) = v?.split_once(' ') {
            keep.insert(t.to_string());
        }
    }
    Ok(keep)
}

fn file_changes(repo: &Path, from: &str, to: &str, path: &str) -> Vec<String> {
    let args =
        if path == "." { vec!["diff", "--name-status", "-M", from, to] } else { vec!["diff", "--name-status", "-M", from, to, "--", path] };
    let out = git(repo, &args).unwrap_or_default();
    let prefix = if path == "." { String::new() } else { format!("{path}/") };
    out.lines()
        .map(|l| {
            let cols: Vec<&str> = l.split('\t').collect();
            let status = cols.first().map(|s| &s[..1]).unwrap_or("?");
            let files: Vec<String> = cols[1..].iter().map(|f| f.strip_prefix(&prefix).unwrap_or(f).to_string()).collect();
            format!("{status} {}", files.join(" → "))
        })
        .collect()
}

fn dir_files(dir: &Path) -> BTreeSet<String> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"))
        .collect()
}

/// `A`/`M`/`D` changes between two directories, in `git diff --name-status` style.
fn dir_changes(from: &Path, to: &Path) -> Vec<String> {
    let (a, b) = (dir_files(from), dir_files(to));
    let mut out = Vec::new();
    for f in a.union(&b) {
        match (a.contains(f), b.contains(f)) {
            (true, false) => out.push(format!("D {f}")),
            (false, true) => out.push(format!("A {f}")),
            _ if std::fs::read(from.join(f)).ok() != std::fs::read(to.join(f)).ok() => out.push(format!("M {f}")),
            _ => {}
        }
    }
    out
}

/// Exclusive per-source-repo lock for operations that rewrite skill directories.
pub fn source_repo_lock(ctx: &Ctx, ws: &SourceRepo) -> Result<std::fs::File> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ctx.paths.locks().join(format!("ws-{}.lock", ws.key())))?;
    f.lock().context("locking source repo")?;
    Ok(f)
}

pub struct UpdateOptions<'a> {
    pub only: Option<&'a str>,
    /// Report what would come in (fetches upstream; changes nothing).
    pub dry_run: bool,
    pub cont: bool,
    pub abort: bool,
}

/// `tricks update`: merge upstream changes into vendored skills (your customizations are
/// kept), left uncommitted for review.
pub fn update(ctx: &Ctx, ws: &SourceRepo, o: &UpdateOptions) -> Result<UpdateReport> {
    if o.dry_run {
        return outdated(ctx, ws, o.only);
    }
    let _lock = source_repo_lock(ctx, ws)?;
    if o.cont || o.abort {
        let names: Vec<String> = match o.only {
            Some(n) => vec![n.to_string()],
            None => pending_merges(ctx, ws)?,
        };
        if names.is_empty() {
            bail!("no upstream update in progress");
        }
        let mut rep = UpdateReport::default();
        for n in names {
            rep.items.push(if o.abort { abort_update(ctx, ws, &n)? } else { continue_update(ctx, ws, &n)? });
        }
        return Ok(rep);
    }
    if let Some(p) = pending_merges(ctx, ws)?.first() {
        bail!("an upstream update of `{p}` is in progress: resolve conflicts, then `tricks update --continue` (or `--abort`)");
    }
    let names: Vec<String> = match o.only {
        Some(n) => {
            ws.skill(n)?;
            vec![n.to_string()]
        }
        None => ws
            .manifest
            .skills
            .iter()
            .filter(|(_, s)| s.upstream.is_some() && !matches!(s.update, Some(Policy::Paused) | Some(Policy::Pinned)))
            .map(|(n, _)| n.clone())
            .collect(),
    };
    let mut rep = UpdateReport::default();
    for n in names {
        let item =
            update_one(ctx, ws, &n).unwrap_or_else(|e| UpdateItem { message: Some(format!("{e:#}")), ..UpdateItem::new(&n, "error") });
        let stop = item.state == "conflicts";
        rep.items.push(item);
        if stop {
            break; // one merge in progress at a time
        }
    }
    Ok(rep)
}

/// `tricks outdated`: what `update` would bring in, per vendored skill (fetches; changes nothing).
pub fn outdated(ctx: &Ctx, ws: &SourceRepo, only: Option<&str>) -> Result<UpdateReport> {
    let mut rep = UpdateReport::default();
    for (name, s) in &ws.manifest.skills {
        if only.is_some_and(|o| o != name) || s.upstream.is_none() || (only.is_none() && s.update == Some(Policy::Paused)) {
            continue;
        }
        let item = match sides(ctx, ws, name, Fetch::IfStale) {
            Ok(Some(sd)) => {
                let changed = !sd.up_to_date();
                UpdateItem {
                    from: Some(sd.base.clone()),
                    to: Some(sd.up_commit.clone()),
                    to_ref: Some(sd.up_label.clone()),
                    risk: if changed { RiskReport::scan_dir(&sd.up_dir).diff_from(&RiskReport::scan_dir(&sd.base_dir)) } else { vec![] },
                    incoming: if changed { sd.incoming } else { vec![] },
                    message: (s.update == Some(Policy::Pinned) && changed)
                        .then(|| "pinned: name it (`tricks update <skill>`) to take the new version".to_string()),
                    ..UpdateItem::new(name, if changed { "update-available" } else { "up-to-date" })
                }
            }
            Ok(None) => continue,
            Err(e) => UpdateItem { message: Some(format!("{e:#}")), ..UpdateItem::new(name, "error") },
        };
        rep.items.push(item);
    }
    Ok(rep)
}

fn update_one(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<UpdateItem> {
    let Some(sd) = sides(ctx, ws, name, Fetch::IfStale)? else {
        return Ok(UpdateItem { message: Some("local original (no upstream)".into()), ..UpdateItem::new(name, "skipped") });
    };
    let dir = ws.skill_dir(name)?;
    let rel = ws.skill(name)?.path.clone();
    let dirty = git(&ws.root, &["status", "--porcelain", "--", &rel])?;
    if !dirty.trim().is_empty() {
        bail!("`{name}` has uncommitted changes; commit or stash them before merging upstream changes");
    }
    if sd.up_to_date() {
        return Ok(UpdateItem {
            from: Some(sd.base.clone()),
            to: Some(sd.up_commit.clone()),
            to_ref: Some(sd.up_label.clone()),
            ..UpdateItem::new(name, "up-to-date")
        });
    }
    let risk = RiskReport::scan_dir(&sd.up_dir).diff_from(&RiskReport::scan_dir(&sd.base_dir));
    // Keep agents on the committed version while the working tree is being merged.
    freeze_dev_placements(ctx, ws, name)?;
    // Back up C, then merge B→U into it.
    let backup = ctx.paths.backups().join(format!("merge-{}-{name}-{}", ws.key(), now()));
    store::copy_dir(&dir, &backup)?;
    let labels = (format!("{name} (yours)"), format!("base {}", crate::id::short_commit(&sd.base)), format!("upstream {}", sd.up_label));
    let outcome = merge::three_way(&sd.base_dir, &dir, &sd.up_dir, (&labels.0, &labels.1, &labels.2))?;
    let item = UpdateItem {
        from: Some(sd.base.clone()),
        to: Some(sd.up_commit.clone()),
        to_ref: Some(sd.up_label.clone()),
        risk,
        incoming: sd.incoming.clone(),
        ..UpdateItem::new(name, "merged")
    };
    if outcome.is_clean() {
        let mut lock = ws.lock.clone();
        let e = lock.skills.entry(name.into()).or_default();
        e.base = Some(sd.up_commit.clone());
        e.base_tree = Some(sd.up_tree.clone());
        if sd.renamed.is_some() {
            e.upstream_path = sd.renamed.clone();
        }
        lock.save(&ws.root)?;
        let _ = store::remove_dir_force(&backup);
        if let Some(r) = &sd.renamed {
            ctx.ui.info(&format!("upstream moved `{name}` to `{r}`; recorded in tricks.lock"));
        }
        Ok(UpdateItem {
            outcome: Some(outcome),
            message: Some(
                "merged into the working tree (uncommitted); review with `git diff` and commit — agents keep the previous version until then"
                    .into(),
            ),
            ..item
        })
    } else {
        ctx.state.conn.execute(
            "INSERT OR REPLACE INTO merges(workspace, skill, target_commit, target_tree, target_path, backup, conflicts, at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                ws.root.to_string_lossy(),
                name,
                sd.up_commit,
                sd.up_tree,
                sd.renamed,
                backup.to_string_lossy(),
                serde_json::to_string(&outcome.conflicts)?,
                now()
            ],
        )?;
        Ok(UpdateItem {
            state: "conflicts".into(),
            outcome: Some(outcome),
            message: Some("resolve the conflicts, then run `tricks update --continue` (or `--abort`)".into()),
            ..item
        })
    }
}

pub fn pending_merges(ctx: &Ctx, ws: &SourceRepo) -> Result<Vec<String>> {
    let mut st = ctx.state.conn.prepare("SELECT skill FROM merges WHERE workspace=?1 ORDER BY at")?;
    let v = st.query_map([ws.root.to_string_lossy()], |r| r.get(0))?.collect::<Result<Vec<String>, _>>()?;
    Ok(v)
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeState {
    pub skill: String,
    pub target_commit: String,
    pub target_tree: String,
    pub target_path: Option<String>,
    pub backup: String,
    pub conflicts: Vec<Conflict>,
}

pub fn merge_state(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<Option<MergeState>> {
    Ok(ctx
        .state
        .conn
        .query_row(
            "SELECT target_commit, target_tree, target_path, backup, conflicts FROM merges WHERE workspace=?1 AND skill=?2",
            params![ws.root.to_string_lossy(), name],
            |r| {
                Ok(MergeState {
                    skill: name.into(),
                    target_commit: r.get(0)?,
                    target_tree: r.get(1)?,
                    target_path: r.get(2)?,
                    backup: r.get(3)?,
                    conflicts: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
                })
            },
        )
        .optional()?)
}

fn continue_update(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<UpdateItem> {
    let st = merge_state(ctx, ws, name)?.with_context(|| format!("no merge in progress for `{name}`"))?;
    let dir = ws.skill_dir(name)?;
    let left = merge::unresolved(&dir, &st.conflicts);
    if !left.is_empty() {
        bail!("unresolved conflicts remain:\n  {}", left.join("\n  "));
    }
    let mut lock = ws.lock.clone();
    let e = lock.skills.entry(name.into()).or_default();
    let from = e.base.clone();
    e.base = Some(st.target_commit.clone());
    e.base_tree = Some(st.target_tree.clone());
    if st.target_path.is_some() {
        e.upstream_path = st.target_path.clone();
    }
    lock.save(&ws.root)?;
    ctx.state.conn.execute("DELETE FROM merges WHERE workspace=?1 AND skill=?2", params![ws.root.to_string_lossy(), name])?;
    let _ = store::remove_dir_force(Path::new(&st.backup));
    Ok(UpdateItem {
        from,
        to: Some(st.target_commit),
        message: Some("update completed (uncommitted); review and commit".into()),
        ..UpdateItem::new(name, "continued")
    })
}

fn abort_update(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<UpdateItem> {
    let st = merge_state(ctx, ws, name)?.with_context(|| format!("no merge in progress for `{name}`"))?;
    let dir = ws.skill_dir(name)?;
    let backup = PathBuf::from(&st.backup);
    if !backup.exists() {
        bail!("backup {} is missing; restore with `git checkout -- {}`", backup.display(), ws.skill(name)?.path);
    }
    deploy::remove_path(&dir)?;
    store::copy_dir(&backup, &dir)?;
    let _ = store::remove_dir_force(&backup);
    ctx.state.conn.execute("DELETE FROM merges WHERE workspace=?1 AND skill=?2", params![ws.root.to_string_lossy(), name])?;
    ctx.state.conn.execute("DELETE FROM meta WHERE key=?1", [snapshot_key(ws, name)])?;
    redeploy(ctx, ws, name)?;
    Ok(UpdateItem { message: Some("restored your version".into()), ..UpdateItem::new(name, "aborted") })
}

// ---------------------------------------------------------------- variants, edit, commit

/// Active variant (branch) for a skill: local override, then manifest `use`.
pub fn active_variant(ws: &SourceRepo, name: &str) -> Result<Option<String>> {
    let wf = WorkFile::load(&ws.root)?;
    if let Some(b) = wf.use_branch.get(name) {
        return Ok(Some(b.clone()).filter(|b| b != "default"));
    }
    Ok(ws.manifest.skills.get(name).and_then(|s| s.use_branch.clone()))
}

fn editing_key(ws: &SourceRepo, name: &str) -> String {
    format!("editing:{}:{name}", ws.root.display())
}

/// The worktree of an experiment branch: `<repo>/.tricks/work/<branch>` (git-ignored), so
/// it sits next to the skills in the editor and inside the repo agents may write to.
fn worktree_path(_ctx: &Ctx, ws: &SourceRepo, branch: &str) -> PathBuf {
    ws.root.join(config::WORK_DIR).join(branch.replace('/', "--"))
}

/// Make sure `.gitignore` ignores the work file and the experiment worktrees.
fn ensure_ignored(root: &Path) -> Result<()> {
    let gi = root.join(".gitignore");
    let mut g = std::fs::read_to_string(&gi).unwrap_or_default();
    let mut changed = false;
    for entry in [config::WORK_FILE, "/.tricks/"] {
        if !g.lines().any(|l| l.trim() == entry || l.trim() == entry.trim_start_matches('/')) {
            if !g.is_empty() && !g.ends_with('\n') {
                g.push('\n');
            }
            g.push_str(&format!("{entry}\n"));
            changed = true;
        }
    }
    if changed {
        std::fs::write(&gi, g)?;
    }
    Ok(())
}

fn snapshot_key(ws: &SourceRepo, name: &str) -> String {
    format!("snapshot:{}:{name}", ws.root.display())
}

fn local_mirror(ws: &SourceRepo) -> Mirror {
    Mirror { source: crate::id::SourceId::new("local", &ws.name), dir: ws.root.clone() }
}

/// Before an upstream merge rewrites the working tree, pin dev deployments to the
/// committed version so agents keep seeing it until the merge result is committed.
fn freeze_dev_placements(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<()> {
    let key = ws.skill_key(name);
    if ctx.state.placements("WHERE skill=?1", &[&key])?.is_empty() {
        return Ok(());
    }
    let head = git::head_commit(&ws.root)?;
    ctx.state.meta_set(&snapshot_key(ws, name), &head)?;
    redeploy(ctx, ws, name)?;
    Ok(())
}

/// What a link of a source repo skill deploys.
#[derive(Debug, Clone)]
pub struct Deployed {
    pub dir: PathBuf,
    pub tree: Option<String>,
    pub commit: Option<String>,
    /// A checkout (edits show immediately) rather than a snapshot in the store.
    pub live: bool,
    /// The branch it comes from (`None` on a detached HEAD).
    pub branch: Option<String>,
}

/// What a link of `name` deploys: with a `pin`, that branch; otherwise the skill's default
/// (its `use` variant, else the working tree).
pub fn deploy_source(ctx: &Ctx, ws: &SourceRepo, name: &str, pin: Option<&str>) -> Result<Deployed> {
    let branch = match pin {
        Some(b) => Some(b.to_string()),
        None => active_variant(ws, name)?,
    };
    match branch {
        Some(b) if git::current_branch(&ws.root).as_deref() != Some(b.as_str()) => deploy_branch(ctx, ws, name, &b),
        _ => deploy_working_tree(ctx, ws, name),
    }
}

/// The skill in the source repo's checkout — or, while an upstream update has rewritten it
/// and is not yet committed, the last committed version.
fn deploy_working_tree(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<Deployed> {
    let rel = ws.skill(name)?.path.clone();
    let branch = git::current_branch(&ws.root);
    if let Some(snap) = ctx.state.meta_get(&snapshot_key(ws, name))? {
        let clean = git(&ws.root, &["status", "--porcelain", "--", &rel]).map(|o| o.trim().is_empty()).unwrap_or(false);
        let merging = merge_state(ctx, ws, name)?.is_some();
        let moved = git::head_commit(&ws.root).map(|h| h != snap).unwrap_or(false);
        if clean && !merging && moved {
            ctx.state.conn.execute("DELETE FROM meta WHERE key=?1", [snapshot_key(ws, name)])?;
        } else {
            let m = local_mirror(ws);
            if let Some(tree) = m.tree_at(&snap, &rel) {
                let dir = store::from_mirror(ctx, &m, &snap, &rel, &tree)?;
                return Ok(Deployed { dir, tree: Some(tree), commit: Some(snap), live: false, branch });
            }
        }
    }
    Ok(Deployed { dir: ws.root.join(&rel), tree: None, commit: None, live: true, branch })
}

/// The skill on another branch: its draft while `tricks edit` has it checked out, else a
/// snapshot of the branch tip (refreshed when the branch moves).
fn deploy_branch(ctx: &Ctx, ws: &SourceRepo, name: &str, branch: &str) -> Result<Deployed> {
    let rel = ws.skill(name)?.path.clone();
    if editing_branch(ctx, ws, name)?.as_deref() == Some(branch) {
        let d = worktree_path(ctx, ws, branch).join(&rel);
        if d.exists() {
            return Ok(Deployed { dir: d, tree: None, commit: None, live: true, branch: Some(branch.into()) });
        }
    }
    let commit = git(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("{branch}^{{commit}}")])
        .ok()
        .filter(|c| !c.is_empty())
        .with_context(|| format!("no branch `{branch}` in source repo {}", ws.name))?;
    let m = local_mirror(ws);
    let tree = m.tree_at(&commit, &rel).with_context(|| format!("branch `{branch}` has no {rel}"))?;
    let dir = store::from_mirror(ctx, &m, &commit, &rel, &tree)?;
    Ok(Deployed { dir, tree: Some(tree), commit: Some(commit), live: false, branch: Some(branch.into()) })
}

/// The branch `tricks edit` has checked out for a skill, if any.
fn editing_branch(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<Option<String>> {
    Ok(ctx.state.meta_get(&editing_key(ws, name))?.filter(|b| !b.is_empty()))
}

/// Place every skill-level link of a source repo skill for `agents_sel` in `scope`. A link
/// already pinned to a branch there keeps its pin; new links follow the default.
pub fn place_skill(
    ctx: &Ctx,
    ws: &SourceRepo,
    name: &str,
    agents_sel: &[&'static Agent],
    scope: &Scope,
    copy: bool,
    shadow: bool,
) -> Result<Vec<crate::state::Placement>> {
    let key = ws.skill_key(name);
    let mut targets: HashMap<Option<String>, Deployed> = HashMap::new();
    let mut out = Vec::new();
    for a in agents_sel {
        let pin =
            ctx.state.placements("WHERE skill=?1 AND agent=?2 AND scope=?3", &[&key, &a.id, &scope.key()])?.into_iter().find_map(|p| p.pin);
        let d = match targets.get(&pin) {
            Some(d) => d.clone(),
            None => {
                let d = deploy_source(ctx, ws, name, pin.as_deref())?;
                targets.insert(pin.clone(), d.clone());
                d
            }
        };
        out.push(deploy::place(
            ctx,
            &PlaceRequest {
                skill: key.clone(),
                origin: "source-repo",
                agent: a,
                scope: scope.clone(),
                name: name.to_string(),
                target: d.dir,
                tree: d.tree,
                commit: d.commit,
                force_copy: copy,
                shadow,
                pin,
                branch: d.branch,
            },
        )?);
    }
    Ok(out)
}

/// Re-point every link of a source repo skill at what it should deploy now: its pinned
/// branch, or the default (working tree, `use` variant, or merge snapshot).
pub fn redeploy(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<Vec<String>> {
    let key = ws.skill_key(name);
    let existing = ctx.state.placements("WHERE skill=?1", &[&key])?;
    let mut targets: HashMap<Option<String>, Option<Deployed>> = HashMap::new();
    let mut out = Vec::new();
    for p in existing {
        let Ok(a) = agents::get(&p.agent) else { continue };
        if !targets.contains_key(&p.pin) {
            let d = match deploy_source(ctx, ws, name, p.pin.as_deref()) {
                Ok(d) => Some(d),
                Err(e) => {
                    ctx.ui.warn(&format!("links of {name} pinned to {}: {e:#}", p.pin.as_deref().unwrap_or("the default")));
                    None
                }
            };
            targets.insert(p.pin.clone(), d);
        }
        let Some(d) = targets[&p.pin].clone() else { continue };
        let placed = deploy::place(
            ctx,
            &PlaceRequest {
                skill: key.clone(),
                origin: if p.origin == "link" { "source-repo" } else { &p.origin },
                agent: a,
                scope: Scope::from_key(&p.scope),
                name: name.to_string(),
                force_copy: p.mode == "copy" && a.follows_links(&d.dir),
                target: d.dir,
                tree: d.tree,
                commit: d.commit,
                shadow: false,
                pin: p.pin.clone(),
                branch: d.branch,
            },
        )?;
        out.push(format!("{} ({})", placed.path, placed.mode));
    }
    Ok(out)
}

/// Links of a skill pinned to `branch` (paths).
fn pinned_to(ctx: &Ctx, ws: &SourceRepo, name: &str, branch: &str) -> Result<Vec<String>> {
    Ok(ctx
        .state
        .placements("WHERE skill=?1 AND pin=?2", &[&ws.skill_key(name), &branch])?
        .into_iter()
        .map(|p| format!("{} ({})", p.path, p.mode))
        .collect())
}

/// Bring links up to date at startup: return them to the working tree once a merge they
/// were frozen for has been committed (with plain `git commit`), and refresh snapshots of
/// branches that have moved. Cheap: only skills with a merge snapshot or snapshot links.
pub fn reconcile(ctx: &Ctx) -> Result<()> {
    let Some(ws) = current(ctx)? else { return Ok(()) };
    let head_branch = git::current_branch(&ws.root);
    for name in ws.manifest.skills.keys() {
        if ctx.state.meta_get(&snapshot_key(&ws, name))?.is_some() {
            redeploy(ctx, &ws, name)?;
            continue;
        }
        let stale = ctx.state.placements("WHERE skill=?1 AND commit_sha IS NOT NULL", &[&ws.skill_key(name)])?.into_iter().any(|p| {
            let branch = match p.pin.clone() {
                Some(b) => Some(b),
                None => active_variant(&ws, name).ok().flatten(),
            };
            match branch {
                Some(b) if head_branch.as_deref() != Some(b.as_str()) => {
                    git(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("{b}^{{commit}}")]).ok().as_deref() != p.commit.as_deref()
                }
                // Snapshot, but the branch is now checked out: deploy the working tree.
                Some(_) => true,
                None => false,
            }
        });
        if stale {
            redeploy(ctx, &ws, name)?;
        }
    }
    Ok(())
}

/// A source repo skill to link: `name` follows the skill's default, `name@branch` is
/// pinned to that branch.
pub fn link_target_for_name(ctx: &Ctx, input: &str) -> Result<Option<LinkTarget>> {
    let Some(ws) = current(ctx)? else { return Ok(None) };
    let (name, pin) = match input.split_once('@') {
        Some((n, b)) => (n, Some(b)),
        None => (input, None),
    };
    if !ws.manifest.skills.contains_key(name) {
        return Ok(None);
    }
    let d = deploy_source(ctx, &ws, name, pin)?;
    Ok(Some(LinkTarget {
        skill: ws.skill_key(name),
        name: name.into(),
        dev: d.live,
        dir: d.dir,
        tree: d.tree,
        commit: d.commit,
        trial: false,
        pin: pin.map(String::from),
        branch: d.branch,
    }))
}

#[derive(Debug, Serialize)]
pub struct EditReport {
    pub name: String,
    pub branch: Option<String>,
    pub path: String,
    pub worktree: Option<String>,
    pub placements: Vec<String>,
}

/// Default branch for `tricks edit <skill>`.
pub fn draft_branch(name: &str) -> String {
    format!("draft/{name}")
}

/// `tricks edit <skill> [-b <branch>]`: start (or continue) an experiment on a branch,
/// checked out in `.tricks/work/<branch>`. Links pinned to the branch
/// (`tricks link <skill>@<branch>`) deploy the draft; other links are left alone.
pub fn edit(ctx: &Ctx, name: &str, branch: Option<&str>) -> Result<EditReport> {
    let ws = require(ctx)?;
    if !ws.manifest.skills.contains_key(name) {
        bail!("`{name}` is not a skill in source repo {}; bring it in first with `tricks vendor` or `tricks create`", ws.name);
    }
    // An explicit branch; else the experiment already under way; else draft/<skill>.
    let b = match branch {
        Some(b) => b.to_string(),
        None => ctx.state.meta_get(&editing_key(&ws, name))?.filter(|b| !b.is_empty()).unwrap_or_else(|| draft_branch(name)),
    };
    let rel = ws.skill(name)?.path.clone();
    ensure_ignored(&ws.root)?;
    let wt = worktree_path(ctx, &ws, &b);
    if !wt.join(".git").exists() {
        std::fs::create_dir_all(wt.parent().unwrap())?;
        let exists = git::git_ok(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]);
        if exists {
            git(&ws.root, &["worktree", "add", "-q", &wt.to_string_lossy(), &b])?;
        } else {
            git(&ws.root, &["worktree", "add", "-q", "-b", &b, &wt.to_string_lossy(), "HEAD"])?;
        }
    }
    if !wt.join(&rel).exists() {
        bail!("branch `{b}` has no {rel} (commit the skill on your main branch first)");
    }
    ctx.state.meta_set(&editing_key(&ws, name), &b)?;
    redeploy(ctx, &ws, name)?;
    let placements = pinned_to(ctx, &ws, name, &b)?;
    Ok(EditReport {
        name: name.to_string(),
        branch: Some(b),
        path: wt.join(&rel).to_string_lossy().to_string(),
        worktree: Some(wt.to_string_lossy().to_string()),
        placements,
    })
}

/// End `edit`: links pinned to the branch deploy its last commit (commit drafts first).
pub fn edit_done(ctx: &Ctx, name: &str) -> Result<EditReport> {
    let ws = require(ctx)?;
    let rel = ws.skill(name)?.path.clone();
    let key = editing_key(&ws, name);
    let Some(branch) = ctx.state.meta_get(&key)? else { bail!("`{name}` is not being edited") };
    let branch = Some(branch).filter(|b| !b.is_empty());
    let dir = match &branch {
        Some(b) => worktree_path(ctx, &ws, b),
        None => ws.root.clone(),
    };
    if git(&dir, &["status", "--porcelain", "--", &rel]).map(|o| !o.trim().is_empty()).unwrap_or(false) {
        ctx.ui.warn(&format!("{} has uncommitted changes in {}", name, dir.display()));
    }
    ctx.state.conn.execute("DELETE FROM meta WHERE key=?1", [key])?;
    redeploy(ctx, &ws, name)?;
    let placements = match &branch {
        Some(b) => pinned_to(ctx, &ws, name, b)?,
        None => vec![],
    };
    Ok(EditReport {
        name: name.to_string(),
        path: dir.join(&rel).to_string_lossy().to_string(),
        worktree: branch.as_ref().map(|_| dir.to_string_lossy().to_string()),
        branch,
        placements,
    })
}

/// Agent environment detection for commit trailers (spec §12).
pub fn detect_agent() -> Option<&'static str> {
    if std::env::var_os("CLAUDECODE").is_some() || std::env::var_os("CLAUDE_CODE_ENTRYPOINT").is_some() {
        return Some("claude-code");
    }
    if std::env::var_os("CODEX_SANDBOX").is_some()
        || std::env::var_os("CODEX_HOME").is_some() && std::env::var_os("CODEX_SESSION_ID").is_some()
    {
        return Some("codex");
    }
    if std::env::var_os("CURSOR_AGENT").is_some() || std::env::var_os("CURSOR_TRACE_ID").is_some() {
        return Some("cursor");
    }
    if std::env::var_os("COPILOT_AGENT").is_some() || std::env::var_os("GITHUB_COPILOT_AGENT").is_some() {
        return Some("copilot");
    }
    None
}

/// `git commit` arguments with a `Tricks-Agent:` trailer when an agent is detected.
fn commit_args<'a>(message: &'a str, trailer: &'a Option<String>) -> Vec<&'a str> {
    let mut args = vec!["commit", "-q", "-m", message];
    if let Some(t) = trailer {
        args.push("--trailer");
        args.push(t);
    }
    args
}

fn agent_trailer() -> Option<String> {
    detect_agent().map(|a| format!("Tricks-Agent: {a}"))
}

#[derive(Debug, Serialize)]
pub struct CommitReport {
    pub name: String,
    pub branch: String,
    pub commit: Option<String>,
    pub agent: Option<String>,
}

/// `tricks edit <skill> --commit -m`: commit the draft on its experiment branch (agents,
/// pre-approved for `edit`, thus only ever commit to experiment branches).
pub fn commit_draft(ctx: &Ctx, name: &str, message: &str) -> Result<CommitReport> {
    let ws = require(ctx)?;
    let rel = ws.skill(name)?.path.clone();
    let branch = ctx
        .state
        .meta_get(&editing_key(&ws, name))?
        .filter(|b| !b.is_empty())
        .with_context(|| format!("`{name}` is not being edited; start an experiment with `tricks edit {name}`"))?;
    let dir = worktree_path(ctx, &ws, &branch);
    git(&dir, &["add", "-A", "--", &rel])?;
    let staged = !git::git_ok(&dir, &["diff", "--cached", "--quiet", "--", &rel]);
    let trailer = agent_trailer();
    let commit = if staged {
        let mut args = commit_args(message, &trailer);
        args.extend(["--", rel.as_str()]);
        git(&dir, &args)?;
        Some(git(&dir, &["rev-parse", "HEAD"])?)
    } else {
        ctx.ui.info(&format!("no changes to {rel} on {branch}"));
        None
    };
    Ok(CommitReport { name: name.into(), branch, commit, agent: detect_agent().map(String::from) })
}

#[derive(Debug, Default, Clone, Copy)]
pub struct MergeOptions<'a> {
    /// Merge the entire branch, not only the skill's folder.
    pub whole_branch: bool,
    /// Open a pull request on the source repo's remote instead of merging locally.
    pub pr: bool,
    pub message: Option<&'a str>,
}

#[derive(Debug, Serialize, Default)]
pub struct MergeReport {
    pub name: String,
    pub branch: String,
    /// The branch merged into (the source repo's current branch).
    pub into: String,
    /// skill (only the skill's folder, one commit) | branch (the whole branch)
    pub mode: String,
    pub commit: Option<String>,
    pub pr_url: Option<String>,
    /// Files left with conflicts to resolve.
    pub conflicts: Vec<String>,
    /// Files outside the skill that the branch also changes (not merged in skill mode).
    pub other_paths: Vec<String>,
    pub placements: Vec<String>,
}

/// `tricks merge <skill>@<branch>`: bring an experiment branch back. By default only the
/// skill's folder, as one commit; `--whole-branch` merges the entire branch; `--pr` opens
/// a pull request on the source repo's remote instead.
pub fn merge_branch(ctx: &Ctx, input: &str, o: &MergeOptions) -> Result<MergeReport> {
    let ws = require(ctx)?;
    let (name, branch) = input.split_once('@').with_context(|| format!("use `tricks merge {input}@<branch>`"))?;
    let rel = ws.skill(name)?.path.clone();
    if !git::git_ok(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]) {
        bail!("no branch `{branch}` in the source repo");
    }
    let into = git::current_branch(&ws.root).context("the source repo is not on a branch (detached HEAD)")?;
    if into == branch {
        bail!("`{branch}` is the current branch");
    }
    if ctx.state.meta_get(&editing_key(&ws, name))?.as_deref() == Some(branch) {
        let wt = worktree_path(ctx, &ws, branch);
        if git(&wt, &["status", "--porcelain", "--", &rel]).map(|o| !o.trim().is_empty()).unwrap_or(false) {
            bail!("the draft of `{name}` on {branch} has uncommitted changes; commit them first (`tricks edit {name} --commit -m \"…\"`)");
        }
    }
    let _lock = source_repo_lock(ctx, &ws)?;
    let base = git(&ws.root, &["merge-base", "HEAD", branch])?;
    let changed: Vec<String> = git(&ws.root, &["diff", "--name-only", &base, branch])?.lines().map(String::from).collect();
    let prefix = format!("{}/", rel.trim_end_matches('/'));
    let (in_skill, other): (Vec<String>, Vec<String>) = changed.into_iter().partition(|f| f.starts_with(&prefix));
    let mode = if o.whole_branch { "branch" } else { "skill" };
    let message = o
        .message
        .map(String::from)
        .unwrap_or_else(|| if o.whole_branch { format!("Merge branch '{branch}'") } else { format!("Merge {branch} into {name}") });
    let mut rep = MergeReport {
        name: name.into(),
        branch: branch.into(),
        into: into.clone(),
        mode: mode.into(),
        other_paths: if o.whole_branch { vec![] } else { other.clone() },
        ..Default::default()
    };
    if !o.whole_branch && in_skill.is_empty() {
        bail!("{branch} has no changes to `{name}` since it branched from {into}");
    }
    let patch = if o.whole_branch { Vec::new() } else { git::git_raw(&ws.root, &["diff", "--binary", &base, branch, "--", &rel])? };
    let trailer = agent_trailer();

    if o.pr {
        let head = if o.whole_branch {
            branch.to_string()
        } else {
            // A branch with only the skill's changes, built off the current branch.
            let head = format!("tricks/merge-{name}-{}", branch.replace('/', "-"));
            let wt = ctx.paths.work().join(ws.key()).join(format!("merge--{}", head.replace('/', "--")));
            if wt.exists() {
                let _ = git(&ws.root, &["worktree", "remove", "--force", &wt.to_string_lossy()]);
            }
            git(&ws.root, &["worktree", "add", "-q", "-B", &head, &wt.to_string_lossy(), "HEAD"])?;
            let applied = git::git_with_input(&wt, &["apply", "--3way", "--whitespace=nowarn"], &patch)
                .and_then(|_| git(&wt, &commit_args(&message, &trailer)));
            let _ = git(&ws.root, &["worktree", "remove", "--force", &wt.to_string_lossy()]);
            if let Err(e) = applied {
                let _ = git(&ws.root, &["branch", "-D", &head]);
                bail!(
                    "the changes to `{name}` on {branch} do not apply cleanly onto {into} ({e:#}); merge locally to resolve the conflicts"
                );
            }
            head
        };
        git(&ws.root, &["push", "-q", "-u", "origin", &head]).context("pushing to the source repo's `origin` remote")?;
        let body = if o.whole_branch {
            format!("Merges the `{branch}` experiment (opened by New Tricks).")
        } else {
            format!("Merges the changes to `{name}` from the `{branch}` experiment (opened by New Tricks).")
        };
        let out = std::process::Command::new("gh")
            .args(["pr", "create", "--base", &into, "--head", &head, "--title", &message, "--body", &body])
            .current_dir(&ws.root)
            .output()
            .context("running gh pr create")?;
        if !out.status.success() {
            bail!("gh pr create failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        rep.pr_url = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
        return Ok(rep);
    }

    if o.whole_branch {
        if !git(&ws.root, &["status", "--porcelain", "--untracked-files=no"])?.trim().is_empty() {
            bail!("the source repo has uncommitted changes; commit or stash them before merging a whole branch");
        }
        // `git merge` takes no trailers: merge without committing, then commit.
        if let Err(e) = git(&ws.root, &["merge", "--no-ff", "--no-commit", "-q", branch]) {
            rep.conflicts = unmerged(&ws.root);
            if rep.conflicts.is_empty() {
                let _ = git(&ws.root, &["merge", "--abort"]);
                return Err(e);
            }
            return Ok(rep);
        }
        git(&ws.root, &commit_args(&message, &trailer))?;
    } else {
        if !git(&ws.root, &["status", "--porcelain", "--", &rel])?.trim().is_empty() {
            bail!("`{name}` has uncommitted changes on {into}; commit or stash them before merging");
        }
        if git::git_with_input(&ws.root, &["apply", "--3way", "--whitespace=nowarn"], &patch).is_err() {
            rep.conflicts = unmerged(&ws.root);
            if rep.conflicts.is_empty() {
                bail!("the changes to `{name}` on {branch} could not be applied onto {into}");
            }
            return Ok(rep);
        }
        if git::git_ok(&ws.root, &["diff", "--cached", "--quiet", "--", &rel]) {
            bail!("the changes to `{name}` on {branch} are already in {into}");
        }
        let mut args = commit_args(&message, &trailer);
        args.extend(["--", rel.as_str()]);
        git(&ws.root, &args)?;
    }
    rep.commit = Some(git::head_commit(&ws.root)?);
    // The experiment is in: stop editing it and stop deploying the branch as a variant.
    if ctx.state.meta_get(&editing_key(&ws, name))?.as_deref() == Some(branch) {
        ctx.state.conn.execute("DELETE FROM meta WHERE key=?1", [editing_key(&ws, name)])?;
    }
    if ws.skill(name)?.use_branch.as_deref() == Some(branch) {
        ws.set_skill_field(name, "use", None)?;
    }
    let wf = ws.root.join(config::WORK_FILE);
    if WorkFile::load(&ws.root)?.use_branch.get(name).map(String::as_str) == Some(branch) {
        let mut doc = config::load_doc(&wf)?;
        config::table_mut(&mut doc, &["use"]).remove(name);
        config::save_doc(&wf, &doc)?;
    }
    // Links pinned to the merged branch now follow the default, which has the changes.
    let ws = SourceRepo::open(&ws.root)?;
    let skills: Vec<String> = if o.whole_branch { ws.manifest.skills.keys().cloned().collect() } else { vec![name.to_string()] };
    for s in &skills {
        let moved = ctx.state.conn.execute("UPDATE placements SET pin=NULL WHERE skill=?1 AND pin=?2", params![ws.skill_key(s), branch])?;
        if s == name {
            rep.placements = redeploy(ctx, &ws, s)?;
        } else if moved > 0 {
            redeploy(ctx, &ws, s)?;
        }
    }
    Ok(rep)
}

fn unmerged(root: &Path) -> Vec<String> {
    git(root, &["diff", "--name-only", "--diff-filter=U"]).map(|o| o.lines().map(String::from).collect()).unwrap_or_default()
}

#[derive(Debug, Serialize)]
pub struct UseReport {
    pub name: String,
    pub variant: Option<String>,
    pub local: bool,
    pub placements: Vec<String>,
}

pub fn use_variant(ctx: &Ctx, input: &str, local: bool, reset: bool) -> Result<UseReport> {
    let ws = require(ctx)?;
    let (name, branch) = match input.split_once('@') {
        Some((n, b)) => (n.to_string(), Some(b.to_string())),
        None if reset => (input.to_string(), None),
        None => bail!("use `name@branch` (or `name --reset`)"),
    };
    ws.skill(&name)?;
    let branch = branch.filter(|b| b != "default");
    if let Some(b) = &branch
        && !git::git_ok(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")])
    {
        bail!("no branch `{b}` in the source repo");
    }
    if local {
        let path = ws.root.join(config::WORK_FILE);
        let mut doc = config::load_doc(&path)?;
        let t = config::table_mut(&mut doc, &["use"]);
        match &branch {
            Some(b) => {
                t.insert(&name, config::str_value(b));
            }
            None => {
                t.insert(&name, config::str_value("default"));
            }
        }
        config::save_doc(&path, &doc)?;
    } else {
        ws.set_skill_field(&name, "use", branch.as_deref())?;
        if reset {
            let path = ws.root.join(config::WORK_FILE);
            if path.exists() {
                let mut doc = config::load_doc(&path)?;
                config::table_mut(&mut doc, &["use"]).remove(&name);
                config::save_doc(&path, &doc)?;
            }
        }
    }
    let ws = SourceRepo::open(&ws.root)?;
    let placements = redeploy(ctx, &ws, &name)?;
    Ok(UseReport { name, variant: branch, local, placements })
}

// ---------------------------------------------------------------- status

#[derive(Debug, Serialize)]
pub struct RepoSkillStatus {
    pub name: String,
    pub path: String,
    pub upstream: Option<String>,
    pub base: Option<String>,
    pub track: Option<String>,
    pub customized: bool,
    pub update_available: Option<String>,
    pub license: Option<config::LicenseRecord>,
    pub lint_errors: usize,
    pub lint_warnings: usize,
    pub branches: Vec<String>,
    pub variant: Option<String>,
    pub editing: Option<String>,
    pub merge_in_progress: bool,
    pub dev_links: usize,
    pub uncommitted: bool,
}

#[derive(Debug, Serialize)]
pub struct RepoStatus {
    pub root: String,
    pub name: String,
    pub branch: Option<String>,
    pub skills: Vec<RepoSkillStatus>,
    pub targets: Vec<String>,
}

pub fn status(ctx: &Ctx, ws: &SourceRepo) -> Result<RepoStatus> {
    let lint = crate::lint::lint_repo(ctx, ws, &[]).ok();
    let branches: Vec<String> =
        git(&ws.root, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]).unwrap_or_default().lines().map(String::from).collect();
    let current = git::current_branch(&ws.root);
    let mut skills = Vec::new();
    for (name, s) in &ws.manifest.skills {
        let locked = ws.lock.skills.get(name);
        let dir = ws.root.join(&s.path);
        let tree = crate::treehash::tree_hash(&dir).ok().flatten();
        let customized = match (locked.and_then(|l| l.base_tree.clone()), &tree) {
            (Some(b), Some(t)) => &b != t,
            _ => false,
        };
        // Update availability from cached mirrors and the index (no network).
        let hosted = upstream_of(ws, name).ok().flatten().filter(|(id, _)| crate::hosted::kind(id).is_some());
        let update_available = if let Some((id, _)) = hosted {
            crate::hosted::indexed_commit(ctx, &id)
                .filter(|c| locked.and_then(|l| l.base.as_deref()) != Some(c.as_str()))
                .map(|c| crate::hosted::label(&c))
        } else if s.upstream.is_some() {
            upstream_of(ws, name).ok().flatten().and_then(|(id, req)| {
                let m = Mirror::open(&ctx.paths, &id.source, false).ok()?;
                let refs = crate::resolve::mirror_refs(&m).ok()?;
                let mut w = Vec::new();
                let u = crate::resolve::resolve_ref(&m, &refs, Some(&req), &mut w).ok()?;
                let ut = m.tree_at(&u.commit, &id.path)?;
                (locked.and_then(|l| l.base.clone()).as_deref() != Some(u.commit.as_str())
                    && locked.and_then(|l| l.base_tree.clone()).as_deref() != Some(ut.as_str()))
                .then_some(u.name)
            })
        } else {
            None
        };
        let skill_branches: Vec<String> = branches
            .iter()
            .filter(|b| Some(b.as_str()) != current.as_deref())
            .filter(|b| {
                let base = current.clone().unwrap_or_else(|| "HEAD".into());
                !git::git_ok(&ws.root, &["diff", "--quiet", &format!("{base}...{b}"), "--", &s.path])
            })
            .cloned()
            .collect();
        let (errs, warns) = lint
            .as_ref()
            .map(|l| {
                let f: Vec<_> = l.findings.iter().filter(|f| &f.skill == name).collect();
                (f.iter().filter(|x| x.severity == "error").count(), f.iter().filter(|x| x.severity == "warning").count())
            })
            .unwrap_or((0, 0));
        let dirty = git(&ws.root, &["status", "--porcelain", "--", &s.path]).map(|o| !o.trim().is_empty()).unwrap_or(false);
        skills.push(RepoSkillStatus {
            name: name.clone(),
            path: s.path.clone(),
            upstream: s.upstream.clone(),
            base: locked.and_then(|l| l.base.clone()),
            track: s.track.clone(),
            customized,
            update_available,
            license: locked.and_then(|l| l.license.clone()),
            lint_errors: errs,
            lint_warnings: warns,
            branches: skill_branches,
            variant: active_variant(ws, name).ok().flatten(),
            editing: ctx.state.meta_get(&editing_key(ws, name)).ok().flatten().filter(|b| !b.is_empty()),
            merge_in_progress: merge_state(ctx, ws, name).ok().flatten().is_some(),
            dev_links: ctx.state.placements("WHERE skill=?1", &[&ws.skill_key(name)]).map(|v| v.len()).unwrap_or(0),
            uncommitted: dirty,
        });
    }
    Ok(RepoStatus {
        root: ws.root.to_string_lossy().to_string(),
        name: ws.name.clone(),
        branch: current,
        skills,
        targets: ws.manifest.publish.targets.keys().cloned().collect(),
    })
}

/// Content of a source repo skill file at B (base), U (upstream) or C (working tree).
pub fn version_file(ctx: &Ctx, ws: &SourceRepo, name: &str, which: &str, rel: &str) -> Result<Option<Vec<u8>>> {
    let rel = rel.trim_start_matches('/');
    match which {
        "working" | "C" => Ok(std::fs::read(ws.skill_dir(name)?.join(rel)).ok()),
        "base" | "B" => Ok(sides(ctx, ws, name, Fetch::Never)?.and_then(|sd| std::fs::read(sd.base_dir.join(rel)).ok())),
        "upstream" | "U" => Ok(sides(ctx, ws, name, Fetch::Never)?.and_then(|sd| std::fs::read(sd.up_dir.join(rel)).ok())),
        "head" => {
            let p = format!("{}/{rel}", ws.skill(name)?.path);
            Ok(git::git_raw(&ws.root, &["show", &format!("HEAD:{p}")]).ok())
        }
        "candidate" | "R" => {
            // What the upstream merge would produce, computed in a scratch copy.
            let dir = candidate_dir(ctx, ws, name)?;
            Ok(std::fs::read(dir.path().join(rel)).ok())
        }
        rev => {
            // A branch, tag or commit of the source repo.
            if !git::git_ok(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")]) {
                bail!("unknown version `{rev}` (a branch or commit of the source repo, or working | head | base)");
            }
            let p = format!("{}/{rel}", ws.skill(name)?.path);
            Ok(git::git_raw(&ws.root, &["show", &format!("{rev}:{p}")]).ok())
        }
    }
}

/// Files of a source repo skill at a git revision.
fn files_at(ws: &SourceRepo, name: &str, rev: &str) -> BTreeSet<String> {
    let Ok(s) = ws.skill(name) else { return BTreeSet::new() };
    let prefix = format!("{}/", s.path.trim_end_matches('/'));
    git(&ws.root, &["ls-tree", "-r", "--name-only", rev, "--", &s.path])
        .map(|o| o.lines().filter_map(|l| l.strip_prefix(&prefix)).map(String::from).collect())
        .unwrap_or_default()
}

/// Merge B → U into a scratch copy of C (the working tree is never touched).
pub fn candidate_dir(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<tempfile::TempDir> {
    let sd = sides(ctx, ws, name, Fetch::Never)?.context("local original: no upstream candidate")?;
    let tmp = tempfile::tempdir()?;
    store::copy_dir(&ws.skill_dir(name)?, tmp.path())?;
    merge::three_way(&sd.base_dir, tmp.path(), &sd.up_dir, ("yours", "base", "upstream"))?;
    Ok(tmp)
}

/// Files that differ between two versions of a source repo skill (for `diff` and the
/// Changes view).
pub fn changed_files(ctx: &Ctx, ws: &SourceRepo, name: &str, from: &str, to: &str) -> Result<Vec<String>> {
    let mut paths = dir_files(&ws.skill_dir(name)?);
    paths.retain(|p| !p.starts_with(".git/"));
    let sd =
        if [from, to].iter().any(|w| matches!(*w, "base" | "B" | "upstream" | "U")) { sides(ctx, ws, name, Fetch::Never)? } else { None };
    if let Some(sd) = &sd {
        paths.extend(dir_files(&sd.base_dir));
        paths.extend(dir_files(&sd.up_dir));
    }
    for rev in [from, to] {
        if !matches!(rev, "working" | "C" | "base" | "B" | "upstream" | "U" | "candidate" | "R") {
            paths.extend(files_at(ws, name, if rev == "head" { "HEAD" } else { rev }));
        }
    }
    // Compute a candidate once rather than per file.
    let cand = if from == "candidate" || to == "candidate" { Some(candidate_dir(ctx, ws, name)?) } else { None };
    if let Some(c) = &cand {
        paths.extend(dir_files(c.path()));
    }
    let read = |which: &str, p: &str| -> Option<Vec<u8>> {
        match (&cand, &sd, which) {
            (Some(c), _, "candidate") => std::fs::read(c.path().join(p)).ok(),
            (_, Some(sd), "base" | "B") => std::fs::read(sd.base_dir.join(p)).ok(),
            (_, Some(sd), "upstream" | "U") => std::fs::read(sd.up_dir.join(p)).ok(),
            _ => version_file(ctx, ws, name, which, p).ok().flatten(),
        }
    };
    let mut out = Vec::new();
    for p in paths {
        if read(from, &p) != read(to, &p) {
            out.push(p);
        }
    }
    Ok(out)
}
