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
use std::collections::BTreeSet;
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
        let gi = root.join(".gitignore");
        let mut g = std::fs::read_to_string(&gi).unwrap_or_default();
        if !g.lines().any(|l| l.trim() == config::WORK_FILE) {
            if !g.is_empty() && !g.ends_with('\n') {
                g.push('\n');
            }
            g.push_str(&format!("{}\n", config::WORK_FILE));
            std::fs::write(&gi, g)?;
        }
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
    /// For a local folder: the upstream skill it was copied from.
    pub upstream: Option<&'a str>,
    /// For a local folder: the upstream revision the copy started from.
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

/// Bring a skill into the source repo to customize it: an upstream skill (git or
/// catalog-hosted; the upstream and base are recorded for `merge`) or a local folder.
pub fn vendor(ctx: &Ctx, ws: &SourceRepo, input: &str, o: &VendorOptions) -> Result<VendorReport> {
    let local = {
        let p = ctx.paths.expand(input);
        if p.is_absolute() { p } else { ctx.opts.cwd.join(p) }
    };
    if local.join("SKILL.md").is_file() && !input.contains("//") {
        return vendor_folder(ctx, ws, &local, o);
    }
    if o.upstream.is_some() || o.base.is_some() {
        bail!("--upstream and --base apply when vendoring a local folder");
    }
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    let hosted_id = SkillId::new(spec.source.clone(), &spec.selector);
    if crate::hosted::kind(&hosted_id).is_some() {
        return vendor_hosted(ctx, ws, &hosted_id, spec.reference.as_deref().filter(|r| *r != "latest"), o);
    }
    let r = resolve_skill(ctx, &spec, Fetch::IfStale)?;
    let name = o.name.map(String::from).unwrap_or_else(|| crate::user::placement_name(&r.name, &r.id));
    let (rel, dest) = destination(ws, &name, o.path)?;
    let store_dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    let lic = crate::inspect::detect_license_in_dir(ctx, &r, &store_dir);
    confirm_licence(ctx, &r.canonical(), &lic)?;
    store::copy_dir(&store_dir, &dest)?;
    let upstream = r.id.to_string();
    let track = track_for(&spec, &r.reference.kind, &r.reference.name);
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
    store::copy_dir(&st.dir, &dest)?;
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

fn vendor_folder(ctx: &Ctx, ws: &SourceRepo, src: &Path, o: &VendorOptions) -> Result<VendorReport> {
    let doc = SkillDoc::parse(&std::fs::read_to_string(src.join("SKILL.md"))?);
    let folder_name = crate::paths::canon(src)?.file_name().unwrap().to_string_lossy().to_string();
    let name = o.name.map(String::from).or(doc.name.clone().filter(|n| valid_skill_name(n))).unwrap_or(folder_name);
    let (rel, dest) = destination(ws, &name, o.path)?;
    let mut locked = RepoLocked::default();
    let mut upstream_id = None;
    if let Some(u) = o.upstream {
        let base = o.base.context("--upstream requires --base <commit> (the upstream revision this copy started from)")?;
        let spec = crate::lookup::spec_from_input(ctx, u)?;
        let r = resolve_skill(ctx, &SkillSpec { reference: Some(base.to_string()), ..spec }, Fetch::IfStale)?;
        locked.base = Some(r.reference.commit.clone());
        locked.base_tree = Some(r.tree.clone());
        upstream_id = Some(r.id.to_string());
    } else if o.base.is_some() {
        bail!("--base needs --upstream");
    }
    store::copy_dir(src, &dest)?;
    let lic = crate::license::detect(&crate::inspect::gather_dir(&dest));
    locked.license = Some(lic.clone());
    let base_out = locked.base.clone();
    record(ws, &name, &rel, upstream_id.as_deref().map(|u| (u, "latest")), locked)?;
    Ok(VendorReport {
        name,
        path: rel,
        upstream: upstream_id,
        base: base_out,
        license: Some(lic),
        risk: RiskReport::scan_dir(&dest).summary(),
    })
}

pub fn new_skill(ws: &SourceRepo, name: &str, description: Option<&str>) -> Result<VendorReport> {
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

// ---------------------------------------------------------------- upstream merges

#[derive(Debug, Serialize)]
pub struct MergeItem {
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

impl MergeItem {
    fn new(name: &str, state: &str) -> MergeItem {
        MergeItem {
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
pub struct MergeReport {
    pub items: Vec<MergeItem>,
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

pub struct MergeOptions<'a> {
    pub only: Option<&'a str>,
    /// Report what would be merged (fetches upstream; changes nothing).
    pub dry_run: bool,
    pub cont: bool,
    pub abort: bool,
}

/// `tricks merge`: bring upstream changes into vendored skills, left uncommitted.
pub fn merge(ctx: &Ctx, ws: &SourceRepo, o: &MergeOptions) -> Result<MergeReport> {
    if o.dry_run {
        return check_upstreams(ctx, ws, o.only);
    }
    let _lock = source_repo_lock(ctx, ws)?;
    if o.cont || o.abort {
        let names: Vec<String> = match o.only {
            Some(n) => vec![n.to_string()],
            None => pending_merges(ctx, ws)?,
        };
        if names.is_empty() {
            bail!("no upstream merge in progress");
        }
        let mut rep = MergeReport::default();
        for n in names {
            rep.items.push(if o.abort { abort_merge(ctx, ws, &n)? } else { continue_merge(ctx, ws, &n)? });
        }
        return Ok(rep);
    }
    if let Some(p) = pending_merges(ctx, ws)?.first() {
        bail!("an upstream merge of `{p}` is in progress: resolve conflicts, then `tricks merge --continue` (or `--abort`)");
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
    let mut rep = MergeReport::default();
    for n in names {
        let item = merge_one(ctx, ws, &n).unwrap_or_else(|e| MergeItem { message: Some(format!("{e:#}")), ..MergeItem::new(&n, "error") });
        let stop = item.state == "conflicts";
        rep.items.push(item);
        if stop {
            break; // one merge in progress at a time
        }
    }
    Ok(rep)
}

/// What `merge` would bring in, per vendored skill (fetches; changes nothing).
fn check_upstreams(ctx: &Ctx, ws: &SourceRepo, only: Option<&str>) -> Result<MergeReport> {
    let mut rep = MergeReport::default();
    for (name, s) in &ws.manifest.skills {
        if only.is_some_and(|o| o != name) || s.upstream.is_none() || (only.is_none() && s.update == Some(Policy::Paused)) {
            continue;
        }
        let item = match sides(ctx, ws, name, Fetch::IfStale) {
            Ok(Some(sd)) => {
                let changed = !sd.up_to_date();
                MergeItem {
                    from: Some(sd.base.clone()),
                    to: Some(sd.up_commit.clone()),
                    to_ref: Some(sd.up_label.clone()),
                    risk: if changed { RiskReport::scan_dir(&sd.up_dir).diff_from(&RiskReport::scan_dir(&sd.base_dir)) } else { vec![] },
                    incoming: if changed { sd.incoming } else { vec![] },
                    message: (s.update == Some(Policy::Pinned) && changed)
                        .then(|| "pinned: merge it by name to take the update".to_string()),
                    ..MergeItem::new(name, if changed { "update-available" } else { "up-to-date" })
                }
            }
            Ok(None) => continue,
            Err(e) => MergeItem { message: Some(format!("{e:#}")), ..MergeItem::new(name, "error") },
        };
        rep.items.push(item);
    }
    Ok(rep)
}

fn merge_one(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<MergeItem> {
    let Some(sd) = sides(ctx, ws, name, Fetch::IfStale)? else {
        return Ok(MergeItem { message: Some("local original (no upstream)".into()), ..MergeItem::new(name, "skipped") });
    };
    let dir = ws.skill_dir(name)?;
    let rel = ws.skill(name)?.path.clone();
    let dirty = git(&ws.root, &["status", "--porcelain", "--", &rel])?;
    if !dirty.trim().is_empty() {
        bail!("`{name}` has uncommitted changes; commit or stash them before merging upstream changes");
    }
    if sd.up_to_date() {
        return Ok(MergeItem {
            from: Some(sd.base.clone()),
            to: Some(sd.up_commit.clone()),
            to_ref: Some(sd.up_label.clone()),
            ..MergeItem::new(name, "up-to-date")
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
    let item = MergeItem {
        from: Some(sd.base.clone()),
        to: Some(sd.up_commit.clone()),
        to_ref: Some(sd.up_label.clone()),
        risk,
        incoming: sd.incoming.clone(),
        ..MergeItem::new(name, "merged")
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
        Ok(MergeItem {
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
        Ok(MergeItem {
            state: "conflicts".into(),
            outcome: Some(outcome),
            message: Some("resolve the conflicts, then run `tricks merge --continue` (or `--abort`)".into()),
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

fn continue_merge(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<MergeItem> {
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
    Ok(MergeItem {
        from,
        to: Some(st.target_commit),
        message: Some("merge completed (uncommitted); review and commit".into()),
        ..MergeItem::new(name, "continued")
    })
}

fn abort_merge(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<MergeItem> {
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
    Ok(MergeItem { message: Some("restored your version".into()), ..MergeItem::new(name, "aborted") })
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

fn worktree_path(ctx: &Ctx, ws: &SourceRepo, branch: &str) -> PathBuf {
    ctx.paths.work().join(ws.key()).join(branch.replace('/', "--"))
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

/// Directory (and optional commit/tree) that a source repo skill currently deploys.
pub fn deploy_source(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<(PathBuf, Option<String>, Option<String>, bool)> {
    let rel = ws.skill(name)?.path.clone();
    let editing = ctx.state.meta_get(&editing_key(ws, name))?;
    if let Some(snap) = ctx.state.meta_get(&snapshot_key(ws, name))? {
        let clean = git(&ws.root, &["status", "--porcelain", "--", &rel]).map(|o| o.trim().is_empty()).unwrap_or(false);
        let merging = merge_state(ctx, ws, name)?.is_some();
        let moved = git::head_commit(&ws.root).map(|h| h != snap).unwrap_or(false);
        if clean && !merging && moved {
            ctx.state.conn.execute("DELETE FROM meta WHERE key=?1", [snapshot_key(ws, name)])?;
        } else if editing.as_deref().map(|b| b.is_empty()).unwrap_or(true) {
            let m = local_mirror(ws);
            if let Some(tree) = m.tree_at(&snap, &rel) {
                let dir = store::from_mirror(ctx, &m, &snap, &rel, &tree)?;
                return Ok((dir, Some(tree), Some(snap), false));
            }
        }
    }
    if let Some(branch) = editing {
        let d = if branch.is_empty() { ws.root.join(&rel) } else { worktree_path(ctx, ws, &branch).join(&rel) };
        return Ok((d, None, None, true));
    }
    match active_variant(ws, name)? {
        None => Ok((ws.root.join(&rel), None, None, true)),
        Some(branch) => {
            let commit = git(&ws.root, &["rev-parse", "--verify", &format!("{branch}^{{commit}}")])
                .with_context(|| format!("variant branch `{branch}` does not exist"))?;
            let m = Mirror { source: crate::id::SourceId::new("local", &ws.name), dir: ws.root.clone() };
            let tree = m.tree_at(&commit, &rel).with_context(|| format!("branch `{branch}` has no {rel}"))?;
            let dir = store::from_mirror(ctx, &m, &commit, &rel, &tree)?;
            Ok((dir, Some(tree), Some(commit), false))
        }
    }
}

/// Place a source repo skill for `agents_sel` in `scope` (dev mode or its active variant).
pub fn place_skill(
    ctx: &Ctx,
    ws: &SourceRepo,
    name: &str,
    agents_sel: &[&'static Agent],
    scope: &Scope,
    copy: bool,
    shadow: bool,
) -> Result<Vec<crate::state::Placement>> {
    let (dir, tree, commit, _dev) = deploy_source(ctx, ws, name)?;
    let mut out = Vec::new();
    for a in agents_sel {
        out.push(deploy::place(
            ctx,
            &PlaceRequest {
                skill: ws.skill_key(name),
                origin: "source-repo",
                agent: a,
                scope: scope.clone(),
                name: name.to_string(),
                target: dir.clone(),
                tree: tree.clone(),
                commit: commit.clone(),
                force_copy: copy,
                shadow,
            },
        )?);
    }
    Ok(out)
}

/// Re-point every placement of a source repo skill at what it should deploy now
/// (dev checkout, branch being edited, active variant, or merge snapshot).
pub fn redeploy(ctx: &Ctx, ws: &SourceRepo, name: &str) -> Result<Vec<String>> {
    let key = ws.skill_key(name);
    let existing = ctx.state.placements("WHERE skill=?1", &[&key])?;
    if existing.is_empty() {
        return Ok(vec![]);
    }
    let (dir, tree, commit, _dev) = deploy_source(ctx, ws, name)?;
    let mut out = Vec::new();
    for p in existing {
        let Ok(a) = agents::get(&p.agent) else { continue };
        let placed = deploy::place(
            ctx,
            &PlaceRequest {
                skill: key.clone(),
                origin: if p.origin == "link" { "source-repo" } else { &p.origin },
                agent: a,
                scope: Scope::from_key(&p.scope),
                name: name.to_string(),
                target: dir.clone(),
                tree: tree.clone(),
                commit: commit.clone(),
                force_copy: p.mode == "copy" && a.follows_links(&dir),
                shadow: false,
            },
        )?;
        out.push(format!("{} ({})", placed.path, placed.mode));
    }
    Ok(out)
}

/// Return dev links to the working tree once a merge they were frozen for has been
/// committed (with plain `git commit`). Cheap: only skills with a merge snapshot.
pub fn reconcile(ctx: &Ctx) -> Result<()> {
    let Some(ws) = current(ctx)? else { return Ok(()) };
    for name in ws.manifest.skills.keys() {
        if ctx.state.meta_get(&snapshot_key(&ws, name))?.is_some() {
            redeploy(ctx, &ws, name)?;
        }
    }
    Ok(())
}

pub fn link_target_for_name(ctx: &Ctx, input: &str) -> Result<Option<LinkTarget>> {
    let Some(ws) = current(ctx)? else { return Ok(None) };
    let (name, variant) = match input.split_once('@') {
        Some((n, v)) => (n, Some(v)),
        None => (input, None),
    };
    if !ws.manifest.skills.contains_key(name) {
        return Ok(None);
    }
    if let Some(v) = variant {
        let rel = ws.skill(name)?.path.clone();
        let commit = git(&ws.root, &["rev-parse", "--verify", &format!("{v}^{{commit}}")])?;
        let m = Mirror { source: crate::id::SourceId::new("local", &ws.name), dir: ws.root.clone() };
        let tree = m.tree_at(&commit, &rel).context("variant lacks the skill")?;
        let dir = store::from_mirror(ctx, &m, &commit, &rel, &tree)?;
        return Ok(Some(LinkTarget {
            skill: ws.skill_key(name),
            name: name.into(),
            dir,
            tree: Some(tree),
            commit: Some(commit),
            dev: false,
            trial: false,
        }));
    }
    let (dir, tree, commit, dev) = deploy_source(ctx, &ws, name)?;
    Ok(Some(LinkTarget { skill: ws.skill_key(name), name: name.into(), dir, tree, commit, dev, trial: false }))
}

#[derive(Debug, Serialize)]
pub struct EditReport {
    pub name: String,
    pub branch: Option<String>,
    pub path: String,
    pub worktree: Option<String>,
    pub placements: Vec<String>,
    pub vendored: bool,
}

pub fn edit(ctx: &Ctx, input: &str, branch: Option<&str>) -> Result<EditReport> {
    let mut ws = require(ctx)?;
    let mut vendored = false;
    let name = if ws.manifest.skills.contains_key(input) {
        input.to_string()
    } else {
        // An upstream skill: vendor it first (copy-on-write).
        let spec = crate::lookup::spec_from_input(ctx, input).with_context(|| format!("`{input}` is not a source repo skill"))?;
        if !ctx.confirm(&format!("`{input}` is not in this source repo. Vendor {spec} into it?"), &[])? {
            bail!("cancelled");
        }
        let r = vendor(ctx, &ws, input, &VendorOptions::default())?;
        ws.reload()?;
        vendored = true;
        r.name
    };
    let rel = ws.skill(&name)?.path.clone();
    let (path, worktree) = match branch {
        None => (ws.root.join(&rel), None),
        Some(b) => {
            let wt = worktree_path(ctx, &ws, b);
            if !wt.join(".git").exists() {
                std::fs::create_dir_all(wt.parent().unwrap())?;
                let exists = git::git_ok(&ws.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")]);
                if exists {
                    git(&ws.root, &["worktree", "add", "-q", &wt.to_string_lossy(), b])?;
                } else {
                    git(&ws.root, &["worktree", "add", "-q", "-b", b, &wt.to_string_lossy(), "HEAD"])?;
                }
            }
            if !wt.join(&rel).exists() {
                bail!("branch `{b}` has no {rel} (commit the skill on your main branch first)");
            }
            (wt.join(&rel), Some(wt))
        }
    };
    ctx.state.meta_set(&editing_key(&ws, &name), branch.unwrap_or(""))?;
    let placements = redeploy(ctx, &ws, &name)?;
    Ok(EditReport {
        name,
        branch: branch.map(String::from),
        path: path.to_string_lossy().to_string(),
        worktree: worktree.map(|w| w.to_string_lossy().to_string()),
        placements,
        vendored,
    })
}

/// End `edit`: the skill deploys its active variant again (commit your changes with git).
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
    let placements = redeploy(ctx, &ws, name)?;
    Ok(EditReport {
        name: name.to_string(),
        path: dir.join(&rel).to_string_lossy().to_string(),
        worktree: branch.as_ref().map(|_| dir.to_string_lossy().to_string()),
        branch,
        placements,
        vendored: false,
    })
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
            editing: ctx
                .state
                .meta_get(&editing_key(ws, name))
                .ok()
                .flatten()
                .map(|b| if b.is_empty() { "main checkout".into() } else { b }),
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
        other => bail!("unknown version `{other}` (base | upstream | working | head | candidate)"),
    }
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
