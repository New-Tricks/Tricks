//! Workspace authoring (spec §10): vendoring with a recorded base, upstream merges left
//! uncommitted for review, branch experiments via worktrees, and dev deployments.

use crate::agents::{self, Agent};
use crate::config::{self, LicenseOverride, Policy, WorkFile, WorkspaceLock, WorkspaceManifest, WsLocked, WsSkill};
use crate::ctx::Ctx;
use crate::deploy::{self, PlaceRequest, Scope};
use crate::git::{self, Mirror, git};
use crate::id::{SkillId, SkillSpec, valid_skill_name};
use crate::links::LinkTarget;
use crate::merge::{self, Conflict, MergeOutcome};
use crate::resolve::{Fetch, resolve_id, resolve_skill};
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
pub struct Workspace {
    pub root: PathBuf,
    pub name: String,
    #[serde(skip)]
    pub manifest: WorkspaceManifest,
    #[serde(skip)]
    pub lock: WorkspaceLock,
}

impl Workspace {
    pub fn open(root: &Path) -> Result<Workspace> {
        let root = crate::paths::canon(root).unwrap_or(root.to_path_buf());
        let manifest = WorkspaceManifest::load(&root)?;
        let lock = WorkspaceLock::load(&root)?;
        let name = manifest
            .workspace
            .name
            .clone()
            .unwrap_or_else(|| root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "workspace".into()));
        Ok(Workspace { root, name, manifest, lock })
    }

    pub fn reload(&mut self) -> Result<()> {
        *self = Workspace::open(&self.root)?;
        Ok(())
    }

    pub fn skill(&self, name: &str) -> Result<&WsSkill> {
        self.manifest.skills.get(name).with_context(|| {
            format!(
                "no skill `{name}` in workspace {} (have: {})",
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
        if !self.manifest.workspace.agents.is_empty() {
            return agents::parse_list(&self.manifest.workspace.agents);
        }
        crate::workbench::default_agents(&crate::workbench::load_manifest(ctx)?)
    }

    fn edit_doc<F: FnOnce(&mut toml_edit::DocumentMut) -> Result<()>>(&self, f: F) -> Result<()> {
        let path = self.root.join(config::WORKSPACE_MANIFEST);
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

pub fn current(ctx: &Ctx) -> Result<Option<Workspace>> {
    match config::find_workspace(&ctx.opts.cwd) {
        Some(root) => Ok(Some(Workspace::open(&root)?)),
        None => Ok(None),
    }
}

pub fn require(ctx: &Ctx) -> Result<Workspace> {
    current(ctx)?.context("not inside a New Tricks workspace (run `tricks init` in a git repository)")
}

/// Registered workspaces (workbench manifest) plus the current one.
pub fn all_workspaces(ctx: &Ctx) -> Result<Vec<Workspace>> {
    let m = crate::workbench::load_manifest(ctx)?;
    let mut roots: BTreeSet<PathBuf> = m.workspaces.values().map(|p| ctx.paths.expand(p)).collect();
    if let Some(r) = config::find_workspace(&ctx.opts.cwd) {
        roots.insert(r);
    }
    Ok(roots.into_iter().filter(|r| r.join(config::WORKSPACE_MANIFEST).is_file()).filter_map(|r| Workspace::open(&r).ok()).collect())
}

pub fn vendored_upstreams(ctx: &Ctx) -> Result<BTreeSet<String>> {
    let mut s = BTreeSet::new();
    for ws in all_workspaces(ctx)? {
        for sk in ws.manifest.skills.values() {
            if let Some(u) = &sk.upstream {
                s.insert(u.clone());
            }
        }
    }
    Ok(s)
}

/// Placement key for a directory inside a workspace skill.
pub fn skill_key_for_dir(dir: &Path) -> Option<String> {
    let root = config::find_workspace(dir)?;
    let ws = Workspace::open(&root).ok()?;
    let rel = dir.strip_prefix(&ws.root).ok()?.to_string_lossy().replace('\\', "/");
    ws.manifest.skills.iter().find(|(_, s)| s.path.trim_end_matches('/') == rel).map(|(n, _)| ws.skill_key(n))
}

// ---------------------------------------------------------------- init

const MANIFEST_TEMPLATE: &str = r#"# New Tricks workspace — https://github.com/new-tricks/tricks
# Skills authored or customized here. Vendored skills record their upstream; the base
# commit lives in tricks.lock.

[workspace]
# agents = ["claude", "codex"]   # agents for dev deployments (default: workbench setting)

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
    let manifest = root.join(config::WORKSPACE_MANIFEST);
    let created = !manifest.exists();
    if created {
        let mut text = MANIFEST_TEMPLATE.to_string();
        if let Some(n) = name {
            text = text.replace("[workspace]\n", &format!("[workspace]\nname = \"{n}\"\n"));
        }
        std::fs::write(&manifest, text)?;
        WorkspaceLock { version: 1, ..Default::default() }.save(&root)?;
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
    let ws = Workspace::open(&root)?;
    // Register in the workbench.
    let path = ctx.paths.workbench_manifest();
    let mut doc = config::load_doc(&path)?;
    let m = crate::workbench::load_manifest(ctx)?;
    let contracted = ctx.paths.contract(&root);
    if !m.workspaces.values().any(|p| ctx.paths.expand(p) == root) {
        let mut key = ws.name.clone();
        let mut i = 2;
        while m.workspaces.contains_key(&key) {
            key = format!("{}-{i}", ws.name);
            i += 1;
        }
        config::table_mut(&mut doc, &["workspaces"]).insert(&key, config::str_value(&contracted));
        config::save_doc(&path, &doc)?;
    }
    let mut placed = Vec::new();
    if agent_skill {
        placed = install_agent_skill(ctx, &ws.agents(ctx)?, &Scope::Project(root.clone()))?;
    }
    Ok(InitReport { root: root.to_string_lossy().to_string(), name: ws.name, created, agent_skill: placed })
}

pub const AGENT_SKILL: &str = include_str!("../assets/new-tricks-skill/SKILL.md");

/// Install the bundled `new-tricks` agent skill (spec §12): at user scope, or into one
/// project (`init --agent-skill` places it in the repository being initialized).
pub fn install_agent_skill(ctx: &Ctx, agents_sel: &[&'static Agent], scope: &Scope) -> Result<Vec<String>> {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("SKILL.md"), AGENT_SKILL)?;
    let (dir, tree) = store::from_dir(ctx, tmp.path())?;
    let mut out = Vec::new();
    for a in agents_sel {
        let p = deploy::place(
            ctx,
            &PlaceRequest {
                skill: "bundled:new-tricks".into(),
                origin: "workbench",
                agent: a,
                scope: scope.clone(),
                name: "new-tricks".into(),
                target: dir.clone(),
                tree: Some(tree.clone()),
                commit: None,
                force_copy: false,
                shadow: false,
            },
        )?;
        out.push(p.path);
    }
    Ok(out)
}

// ---------------------------------------------------------------- vendor / import / new

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

pub fn vendor(ctx: &Ctx, ws: &Workspace, input: &str, name: Option<&str>, path: Option<&str>) -> Result<VendorReport> {
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    let r = resolve_skill(ctx, &spec, Fetch::IfStale)?;
    let name = name.map(String::from).unwrap_or_else(|| crate::workbench::placement_name(&r.name, &r.id));
    if !valid_skill_name(&name) {
        bail!("`{name}` is not a valid skill name (lowercase letters, digits and single hyphens)");
    }
    if ws.manifest.skills.contains_key(&name) {
        bail!("workspace already has a skill named `{name}`");
    }
    let rel = path.map(|p| p.trim_matches('/').to_string()).unwrap_or_else(|| format!("skills/{name}"));
    let dest = ws.root.join(&rel);
    if dest.exists() {
        bail!("{} already exists", dest.display());
    }
    let store_dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    let lic = crate::inspect::detect_license_in_dir(ctx, &r, &store_dir);
    let class = crate::license::Class::parse(&lic.class);
    if class == crate::license::Class::Block || class == crate::license::Class::NonCommercial {
        let details = vec![
            format!("licence: {} ({}, via {})", lic.spdx.as_deref().unwrap_or("none"), lic.class, lic.source),
            "its terms may prohibit modification or redistribution; you are responsible for complying".to_string(),
        ];
        if !ctx.confirm(&format!("Vendor {} anyway?", r.canonical()), &details)? {
            bail!("cancelled");
        }
    }
    store::copy_dir(&store_dir, &dest)?;
    let risk = RiskReport::scan_dir(&dest).summary();
    let upstream = r.id.to_string();
    let track = track_for(&spec, &r.reference.kind, &r.reference.name);
    ws.edit_doc(|doc| {
        let t = config::table_mut(doc, &["skills", &name]);
        t.set_implicit(false);
        t.insert("path", config::str_value(&rel));
        t.insert("upstream", config::str_value(&upstream));
        t.insert("track", config::str_value(&track));
        t.insert("update", config::str_value("review"));
        Ok(())
    })?;
    let mut lock = ws.lock.clone();
    lock.skills.insert(
        name.clone(),
        WsLocked {
            base: Some(r.reference.commit.clone()),
            base_tree: Some(r.tree.clone()),
            upstream_path: None,
            license: Some(lic.clone()),
        },
    );
    lock.save(&ws.root)?;
    Ok(VendorReport { name, path: rel, upstream: Some(upstream), base: Some(r.reference.commit), license: Some(lic), risk })
}

pub fn import(
    ctx: &Ctx,
    ws: &Workspace,
    folder: &str,
    name: Option<&str>,
    upstream: Option<&str>,
    base: Option<&str>,
) -> Result<VendorReport> {
    let src = ctx.paths.expand(folder);
    let src = if src.is_absolute() { src } else { ctx.opts.cwd.join(src) };
    if !src.join("SKILL.md").is_file() {
        bail!("{} has no SKILL.md", src.display());
    }
    let doc = SkillDoc::parse(&std::fs::read_to_string(src.join("SKILL.md"))?);
    let folder_name = crate::paths::canon(&src)?.file_name().unwrap().to_string_lossy().to_string();
    let name = name.map(String::from).or(doc.name.clone().filter(|n| valid_skill_name(n))).unwrap_or(folder_name);
    if !valid_skill_name(&name) {
        bail!("`{name}` is not a valid skill name; pass --name");
    }
    if ws.manifest.skills.contains_key(&name) {
        bail!("workspace already has a skill named `{name}`");
    }
    let rel = format!("skills/{name}");
    let dest = ws.root.join(&rel);
    if dest.exists() {
        bail!("{} already exists", dest.display());
    }
    let mut locked = WsLocked::default();
    let mut upstream_id = None;
    if let Some(u) = upstream {
        let base = base.context("--upstream requires --base <commit> (the upstream revision this copy started from)")?;
        let spec = crate::lookup::spec_from_input(ctx, u)?;
        let r = resolve_skill(ctx, &SkillSpec { reference: Some(base.to_string()), ..spec }, Fetch::IfStale)?;
        locked.base = Some(r.reference.commit.clone());
        locked.base_tree = Some(r.tree.clone());
        upstream_id = Some(r.id.to_string());
    }
    store::copy_dir(&src, &dest)?;
    let inputs = crate::inspect::gather_dir(&dest);
    let lic = crate::license::detect(&inputs);
    locked.license = Some(lic.clone());
    ws.edit_doc(|doc| {
        let t = config::table_mut(doc, &["skills", &name]);
        t.set_implicit(false);
        t.insert("path", config::str_value(&rel));
        if let Some(u) = &upstream_id {
            t.insert("upstream", config::str_value(u));
            t.insert("track", config::str_value("latest"));
            t.insert("update", config::str_value("review"));
        }
        Ok(())
    })?;
    let mut lock = ws.lock.clone();
    let base_out = locked.base.clone();
    lock.skills.insert(name.clone(), locked);
    lock.save(&ws.root)?;
    Ok(VendorReport {
        name,
        path: rel,
        upstream: upstream_id,
        base: base_out,
        license: Some(lic),
        risk: RiskReport::scan_dir(&dest).summary(),
    })
}

pub fn new_skill(ws: &Workspace, name: &str, description: Option<&str>) -> Result<VendorReport> {
    if !valid_skill_name(name) {
        bail!("`{name}` is not a valid skill name (lowercase letters, digits and single hyphens; max 64)");
    }
    if ws.manifest.skills.contains_key(name) {
        bail!("workspace already has a skill named `{name}`");
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

// ---------------------------------------------------------------- upstream updates

#[derive(Debug, Serialize)]
pub struct WsUpdateItem {
    pub name: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub to_ref: Option<String>,
    /// up-to-date | merged | conflicts | continued | aborted | skipped | error
    pub state: String,
    pub outcome: Option<MergeOutcome>,
    pub risk: Vec<String>,
    pub incoming: Vec<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct WsUpdateReport {
    pub items: Vec<WsUpdateItem>,
}

fn upstream_of(ws: &Workspace, name: &str) -> Result<Option<(SkillId, String)>> {
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

pub fn outdated(ctx: &Ctx, ws: &Workspace) -> Result<WsUpdateReport> {
    let mut rep = WsUpdateReport::default();
    for (name, s) in &ws.manifest.skills {
        if s.upstream.is_none() || s.update == Some(Policy::Paused) {
            continue;
        }
        let Some((id, req)) = upstream_of(ws, name)? else { continue };
        let base = ws.lock.skills.get(name).and_then(|l| l.base.clone());
        match resolve_id(ctx, &id, &req, Fetch::IfStale) {
            Ok(u) => {
                let changed = base.as_deref() != Some(u.reference.commit.as_str())
                    && ws.lock.skills.get(name).and_then(|l| l.base_tree.clone()).as_deref() != Some(u.tree.as_str());
                let incoming = base
                    .as_deref()
                    .map(|b| crate::workbench::file_changes(&u.mirror.dir, b, &u.reference.commit, &id.path))
                    .unwrap_or_default();
                rep.items.push(WsUpdateItem {
                    name: name.clone(),
                    from: base,
                    to: Some(u.reference.commit.clone()),
                    to_ref: Some(u.reference.name.clone()),
                    state: if changed { "update-available".into() } else { "up-to-date".into() },
                    outcome: None,
                    risk: vec![],
                    incoming: if changed { incoming } else { vec![] },
                    message: None,
                });
            }
            Err(e) => rep.items.push(WsUpdateItem {
                name: name.clone(),
                from: base,
                to: None,
                to_ref: None,
                state: "error".into(),
                outcome: None,
                risk: vec![],
                incoming: vec![],
                message: Some(format!("{e:#}")),
            }),
        }
    }
    Ok(rep)
}

/// Exclusive per-workspace lock for operations that rewrite skill directories.
pub fn workspace_lock(ctx: &Ctx, ws: &Workspace) -> Result<std::fs::File> {
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(ctx.paths.locks().join(format!("ws-{}.lock", ws.key())))?;
    f.lock().context("locking workspace")?;
    Ok(f)
}

pub fn update(ctx: &Ctx, ws: &Workspace, only: Option<&str>, cont: bool, abort: bool) -> Result<WsUpdateReport> {
    let _lock = workspace_lock(ctx, ws)?;
    if cont || abort {
        let names: Vec<String> = match only {
            Some(n) => vec![n.to_string()],
            None => pending_merges(ctx, ws)?,
        };
        if names.is_empty() {
            bail!("no upstream merge in progress");
        }
        let mut rep = WsUpdateReport::default();
        for n in names {
            rep.items.push(if abort { abort_merge(ctx, ws, &n)? } else { continue_merge(ctx, ws, &n)? });
        }
        return Ok(rep);
    }
    if let Some(p) = pending_merges(ctx, ws)?.first() {
        bail!("an upstream merge of `{p}` is in progress: resolve conflicts, then `tricks update --continue` (or `--abort`)");
    }
    let names: Vec<String> = match only {
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
    let mut rep = WsUpdateReport::default();
    for n in names {
        let item = match update_one(ctx, ws, &n) {
            Ok(i) => i,
            Err(e) => WsUpdateItem {
                name: n.clone(),
                from: None,
                to: None,
                to_ref: None,
                state: "error".into(),
                outcome: None,
                risk: vec![],
                incoming: vec![],
                message: Some(format!("{e:#}")),
            },
        };
        let stop = item.state == "conflicts";
        rep.items.push(item);
        if stop {
            break; // one merge in progress at a time
        }
    }
    Ok(rep)
}

fn update_one(ctx: &Ctx, ws: &Workspace, name: &str) -> Result<WsUpdateItem> {
    let Some((mut id, req)) = upstream_of(ws, name)? else {
        return Ok(WsUpdateItem {
            name: name.into(),
            from: None,
            to: None,
            to_ref: None,
            state: "skipped".into(),
            outcome: None,
            risk: vec![],
            incoming: vec![],
            message: Some("local original (no upstream)".into()),
        });
    };
    let locked = ws.lock.skills.get(name).cloned().unwrap_or_default();
    let base = locked.base.clone().context("no base commit recorded in tricks.lock; re-vendor or import with --upstream/--base")?;
    let dir = ws.skill_dir(name)?;
    let rel = ws.skill(name)?.path.clone();
    let dirty = git(&ws.root, &["status", "--porcelain", "--", &rel])?;
    if !dirty.trim().is_empty() {
        bail!("`{name}` has uncommitted changes; commit or stash them before merging upstream changes");
    }
    // Resolve U; follow an upstream rename if the path disappeared.
    let mirror = crate::resolve::open_mirror(ctx, &id.source, Fetch::IfStale)?;
    let refs = crate::resolve::mirror_refs(&mirror)?;
    let mut warns = Vec::new();
    let u_ref = crate::resolve::resolve_ref(&mirror, &refs, Some(&req), &mut warns)?;
    mirror.ensure_commit(&base)?;
    let mut renamed = None;
    let u_tree = match mirror.tree_at(&u_ref.commit, &id.path) {
        Some(t) => t,
        None => match mirror.renamed_path(&base, &u_ref.commit, &id.path) {
            Some(newp) => {
                let t = mirror.tree_at(&u_ref.commit, &newp).context("renamed path has no tree")?;
                renamed = Some(newp.clone());
                id.path = newp;
                t
            }
            None => bail!("upstream no longer contains {} (deleted or moved); the vendored copy is kept", id),
        },
    };
    if u_ref.commit == base || Some(&u_tree) == locked.base_tree.as_ref() {
        return Ok(WsUpdateItem {
            name: name.into(),
            from: Some(base.clone()),
            to: Some(u_ref.commit.clone()),
            to_ref: Some(u_ref.name.clone()),
            state: "up-to-date".into(),
            outcome: None,
            risk: vec![],
            incoming: vec![],
            message: None,
        });
    }
    let base_path = locked
        .upstream_path
        .clone()
        .unwrap_or_else(|| SkillId::parse_canonical(ws.skill(name).unwrap().upstream.as_ref().unwrap()).unwrap().path);
    let b_tree = mirror.tree_at(&base, &base_path).context("base revision missing the skill directory")?;
    let b_dir = store::from_mirror(ctx, &mirror, &base, &base_path, &b_tree)?;
    let u_dir = store::from_mirror(ctx, &mirror, &u_ref.commit, &id.path, &u_tree)?;
    let risk = RiskReport::scan_dir(&u_dir).diff_from(&RiskReport::scan_dir(&b_dir));
    let incoming = crate::workbench::file_changes(&mirror.dir, &base, &u_ref.commit, &id.path);

    // Keep agents on the committed version while the working tree is being merged.
    freeze_dev_placements(ctx, ws, name)?;
    // Back up C, then merge B→U into it.
    let backup = ctx.paths.backups().join(format!("merge-{}-{name}-{}", ws.key(), now()));
    store::copy_dir(&dir, &backup)?;
    let labels = (format!("{name} (yours)"), format!("base {}", &base[..9]), format!("upstream {}", u_ref.name));
    let outcome = merge::three_way(&b_dir, &dir, &u_dir, (&labels.0, &labels.1, &labels.2))?;
    let mut lock = ws.lock.clone();
    if outcome.is_clean() {
        let e = lock.skills.entry(name.into()).or_default();
        e.base = Some(u_ref.commit.clone());
        e.base_tree = Some(u_tree.clone());
        if renamed.is_some() {
            e.upstream_path = renamed.clone();
        }
        lock.save(&ws.root)?;
        let _ = store::remove_dir_force(&backup);
        if let Some(r) = &renamed {
            ctx.ui.info(&format!("upstream moved `{base_path}` to `{r}`; recorded in tricks.lock"));
        }
        Ok(WsUpdateItem {
            name: name.into(),
            from: Some(base),
            to: Some(u_ref.commit.clone()),
            to_ref: Some(u_ref.name.clone()),
            state: "merged".into(),
            outcome: Some(outcome),
            risk,
            incoming,
            message: Some(format!(
                "merged into the working tree (uncommitted); review with `git diff`, then `tricks commit {name} -m \"…\"` (agents keep the previous version until then)"
            )),
        })
    } else {
        ctx.state.conn.execute(
            "INSERT OR REPLACE INTO merges(workspace, skill, target_commit, target_tree, target_path, backup, conflicts, at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![ws.root.to_string_lossy(), name, u_ref.commit, u_tree, renamed, backup.to_string_lossy(), serde_json::to_string(&outcome.conflicts)?, now()],
        )?;
        Ok(WsUpdateItem {
            name: name.into(),
            from: Some(base),
            to: Some(u_ref.commit.clone()),
            to_ref: Some(u_ref.name.clone()),
            state: "conflicts".into(),
            outcome: Some(outcome),
            risk,
            incoming,
            message: Some("resolve the conflicts, then run `tricks update --continue` (or `--abort`)".into()),
        })
    }
}

pub fn pending_merges(ctx: &Ctx, ws: &Workspace) -> Result<Vec<String>> {
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

pub fn merge_state(ctx: &Ctx, ws: &Workspace, name: &str) -> Result<Option<MergeState>> {
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

fn continue_merge(ctx: &Ctx, ws: &Workspace, name: &str) -> Result<WsUpdateItem> {
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
    Ok(WsUpdateItem {
        name: name.into(),
        from,
        to: Some(st.target_commit),
        to_ref: None,
        state: "continued".into(),
        outcome: None,
        risk: vec![],
        incoming: vec![],
        message: Some("merge completed (uncommitted); review and commit".into()),
    })
}

fn abort_merge(ctx: &Ctx, ws: &Workspace, name: &str) -> Result<WsUpdateItem> {
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
    redeploy(ctx, ws, name, None)?;
    Ok(WsUpdateItem {
        name: name.into(),
        from: None,
        to: None,
        to_ref: None,
        state: "aborted".into(),
        outcome: None,
        risk: vec![],
        incoming: vec![],
        message: Some("restored your version".into()),
    })
}

// ---------------------------------------------------------------- variants, edit, commit

/// Active variant (branch) for a skill: local override, then manifest `use`.
pub fn active_variant(ws: &Workspace, name: &str) -> Result<Option<String>> {
    let wf = WorkFile::load(&ws.root)?;
    if let Some(b) = wf.use_branch.get(name) {
        return Ok(Some(b.clone()).filter(|b| b != "default"));
    }
    Ok(ws.manifest.skills.get(name).and_then(|s| s.use_branch.clone()))
}

fn editing_key(ws: &Workspace, name: &str) -> String {
    format!("editing:{}:{name}", ws.root.display())
}

fn worktree_path(ctx: &Ctx, ws: &Workspace, branch: &str) -> PathBuf {
    ctx.paths.work().join(ws.key()).join(branch.replace('/', "--"))
}

fn snapshot_key(ws: &Workspace, name: &str) -> String {
    format!("snapshot:{}:{name}", ws.root.display())
}

fn local_mirror(ws: &Workspace) -> Mirror {
    Mirror { source: crate::id::SourceId::new("local", &ws.name), dir: ws.root.clone() }
}

/// Before an upstream merge rewrites the working tree, pin dev deployments to the
/// committed version so agents keep seeing it until the merge result is committed.
fn freeze_dev_placements(ctx: &Ctx, ws: &Workspace, name: &str) -> Result<()> {
    let key = ws.skill_key(name);
    if ctx.state.placements("WHERE skill=?1", &[&key])?.is_empty() {
        return Ok(());
    }
    let head = git::head_commit(&ws.root)?;
    ctx.state.meta_set(&snapshot_key(ws, name), &head)?;
    redeploy(ctx, ws, name, None)?;
    Ok(())
}

/// Directory (and optional commit/tree) that a workspace skill currently deploys.
pub fn deploy_source(ctx: &Ctx, ws: &Workspace, name: &str) -> Result<(PathBuf, Option<String>, Option<String>, bool)> {
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

fn skill_placement_name(ws: &Workspace, name: &str) -> String {
    let _ = ws;
    name.to_string()
}

/// (Re)deploy a workspace skill's dev placements for the workspace agents.
pub fn redeploy(ctx: &Ctx, ws: &Workspace, name: &str, agents_sel: Option<&[&'static Agent]>) -> Result<Vec<String>> {
    let key = ws.skill_key(name);
    let existing = ctx.state.placements("WHERE skill=?1 AND origin='workspace'", &[&key])?;
    let agents_sel: Vec<&'static Agent> = match agents_sel {
        Some(a) => a.to_vec(),
        None if !existing.is_empty() => existing.iter().filter_map(|p| agents::get(&p.agent).ok()).collect(),
        None => return Ok(vec![]),
    };
    let (dir, tree, commit, _dev) = deploy_source(ctx, ws, name)?;
    let mut out = Vec::new();
    for a in agents_sel {
        let p = deploy::place(
            ctx,
            &PlaceRequest {
                skill: key.clone(),
                origin: "workspace",
                agent: a,
                scope: Scope::Global,
                name: skill_placement_name(ws, name),
                target: dir.clone(),
                tree: tree.clone(),
                commit: commit.clone(),
                force_copy: false,
                shadow: false,
            },
        )?;
        out.push(format!("{} ({})", p.path, p.mode));
    }
    // Test links of this skill into projects follow the same variant.
    for p in ctx.state.placements("WHERE skill=?1 AND origin='link'", &[&key])? {
        if let Ok(a) = agents::get(&p.agent) {
            deploy::place(
                ctx,
                &PlaceRequest {
                    skill: key.clone(),
                    origin: "link",
                    agent: a,
                    scope: Scope::from_key(&p.scope),
                    name: skill_placement_name(ws, name),
                    target: dir.clone(),
                    tree: tree.clone(),
                    commit: commit.clone(),
                    force_copy: p.mode == "copy" && a.follows_links(&dir),
                    shadow: false,
                },
            )?;
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct WsInstallReport {
    pub workspace: String,
    pub skills: Vec<(String, Vec<String>)>,
}

/// In a workspace, `install` links every workspace skill in dev mode (spec §8).
pub fn install_dev(ctx: &Ctx, ws: &Workspace, agent_names: &[String]) -> Result<WsInstallReport> {
    let agents_sel = if agent_names.is_empty() { ws.agents(ctx)? } else { agents::parse_list(agent_names)? };
    let mut skills = Vec::new();
    for name in ws.manifest.skills.keys() {
        match redeploy(ctx, ws, name, Some(&agents_sel)) {
            Ok(p) => skills.push((name.clone(), p)),
            Err(e) => skills.push((name.clone(), vec![format!("error: {e:#}")])),
        }
    }
    Ok(WsInstallReport { workspace: ws.name.clone(), skills })
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
        }));
    }
    let (dir, tree, commit, dev) = deploy_source(ctx, &ws, name)?;
    Ok(Some(LinkTarget { skill: ws.skill_key(name), name: name.into(), dir, tree, commit, dev }))
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
        let spec = crate::lookup::spec_from_input(ctx, input).with_context(|| format!("`{input}` is not a workspace skill"))?;
        if !ctx.confirm(&format!("`{input}` is not in this workspace. Vendor {spec} into it?"), &[])? {
            bail!("cancelled");
        }
        let r = vendor(ctx, &ws, input, None, None)?;
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
    let placements = redeploy(ctx, &ws, &name, None)?;
    Ok(EditReport {
        name,
        branch: branch.map(String::from),
        path: path.to_string_lossy().to_string(),
        worktree: worktree.map(|w| w.to_string_lossy().to_string()),
        placements,
        vendored,
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

#[derive(Debug, Serialize)]
pub struct CommitReport {
    pub name: String,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub agent: Option<String>,
    pub placements: Vec<String>,
}

pub fn commit(ctx: &Ctx, name: &str, message: &str) -> Result<CommitReport> {
    let ws = require(ctx)?;
    let rel = ws.skill(name)?.path.clone();
    let branch = ctx.state.meta_get(&editing_key(&ws, name))?.filter(|b| !b.is_empty());
    let dir = match &branch {
        Some(b) => worktree_path(ctx, &ws, b),
        None => ws.root.clone(),
    };
    git(&dir, &["add", "-A", "--", &rel])?;
    let staged = !git::git_ok(&dir, &["diff", "--cached", "--quiet", "--", &rel]);
    let agent = detect_agent();
    let mut commit = None;
    if staged {
        let mut args = vec!["commit", "-q", "-m", message];
        let trailer;
        if let Some(a) = agent {
            trailer = format!("Tricks-Agent: {a}");
            args.push("--trailer");
            args.push(&trailer);
        }
        args.push("--");
        args.push(&rel);
        git(&dir, &args)?;
        commit = Some(git(&dir, &["rev-parse", "HEAD"])?);
    } else {
        ctx.ui.info(&format!("no changes to {rel}"));
    }
    ctx.state.conn.execute("DELETE FROM meta WHERE key=?1", [editing_key(&ws, name)])?;
    let placements = redeploy(ctx, &ws, name, None)?;
    Ok(CommitReport { name: name.into(), branch, commit, agent: agent.map(String::from), placements })
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
        bail!("no branch `{b}` in the workspace");
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
    let ws = Workspace::open(&ws.root)?;
    let placements = redeploy(ctx, &ws, &name, None)?;
    Ok(UseReport { name, variant: branch, local, placements })
}

pub fn set_license_override(ws: &Workspace, name: &str, justification: &str) -> Result<()> {
    ws.skill(name)?;
    let ov = LicenseOverride { justification: justification.to_string() };
    ws.edit_doc(|doc| {
        config::table_mut(doc, &["skills", name]).insert("license-override", config::to_inline(&ov)?);
        Ok(())
    })
}

// ---------------------------------------------------------------- status

#[derive(Debug, Serialize)]
pub struct WsSkillStatus {
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
pub struct WsStatus {
    pub root: String,
    pub name: String,
    pub branch: Option<String>,
    pub skills: Vec<WsSkillStatus>,
    pub targets: Vec<String>,
}

pub fn status(ctx: &Ctx, ws: &Workspace) -> Result<WsStatus> {
    let lint = crate::lint::lint_workspace(ctx, ws, &[]).ok();
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
        // Update availability from the cached mirror (no network).
        let update_available = if s.upstream.is_some() {
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
        skills.push(WsSkillStatus {
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
    Ok(WsStatus {
        root: ws.root.to_string_lossy().to_string(),
        name: ws.name.clone(),
        branch: current,
        skills,
        targets: ws.manifest.publish.targets.keys().cloned().collect(),
    })
}

/// Content of a workspace skill file at B (base), U (upstream) or C (working tree).
pub fn version_file(ctx: &Ctx, ws: &Workspace, name: &str, which: &str, rel: &str) -> Result<Option<Vec<u8>>> {
    let rel = rel.trim_start_matches('/');
    match which {
        "working" | "C" => Ok(std::fs::read(ws.skill_dir(name)?.join(rel)).ok()),
        "base" | "B" => {
            let Some((mut id, _)) = upstream_of(ws, name)? else { return Ok(None) };
            let l = ws.lock.skills.get(name).cloned().unwrap_or_default();
            let Some(base) = l.base else { return Ok(None) };
            if l.upstream_path.is_none() {
                id = SkillId::parse_canonical(ws.skill(name)?.upstream.as_ref().unwrap())?;
            }
            let m = crate::resolve::open_mirror(ctx, &id.source, Fetch::Never)?;
            let p = if id.path == "." { rel.to_string() } else { format!("{}/{rel}", id.path) };
            Ok(m.read_file(&base, &p).ok())
        }
        "upstream" | "U" => {
            let Some((id, req)) = upstream_of(ws, name)? else { return Ok(None) };
            let u = resolve_id(ctx, &id, &req, Fetch::Never)?;
            let p = if id.path == "." { rel.to_string() } else { format!("{}/{rel}", id.path) };
            Ok(u.mirror.read_file(&u.reference.commit, &p).ok())
        }
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
pub fn candidate_dir(ctx: &Ctx, ws: &Workspace, name: &str) -> Result<tempfile::TempDir> {
    let (id, req) = upstream_of(ws, name)?.context("local original: no upstream candidate")?;
    let l = ws.lock.skills.get(name).cloned().unwrap_or_default();
    let base = l.base.clone().context("no base recorded")?;
    let base_path = match &l.upstream_path {
        Some(_) => SkillId::parse_canonical(ws.skill(name)?.upstream.as_ref().unwrap())?.path,
        None => id.path.clone(),
    };
    let u = resolve_id(ctx, &id, &req, Fetch::Never)?;
    u.mirror.ensure_commit(&base)?;
    let b_tree = u.mirror.tree_at(&base, &base_path).context("base revision lacks the skill")?;
    let b_dir = store::from_mirror(ctx, &u.mirror, &base, &base_path, &b_tree)?;
    let u_dir = store::from_mirror(ctx, &u.mirror, &u.reference.commit, &u.id.path, &u.tree)?;
    let tmp = tempfile::tempdir()?;
    store::copy_dir(&ws.skill_dir(name)?, tmp.path())?;
    merge::three_way(&b_dir, tmp.path(), &u_dir, ("yours", "base", "upstream"))?;
    Ok(tmp)
}

/// Files that differ between two versions of a workspace skill (for the Changes view).
pub fn changed_files(ctx: &Ctx, ws: &Workspace, name: &str, from: &str, to: &str) -> Result<Vec<String>> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    let dir = ws.skill_dir(name)?;
    for e in walkdir::WalkDir::new(&dir).into_iter().filter_entry(|e| e.file_name() != ".git").flatten() {
        if e.file_type().is_file() {
            paths.insert(e.path().strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
    if let Some((id, req)) = upstream_of(ws, name)?
        && let Ok(m) = crate::resolve::open_mirror(ctx, &id.source, Fetch::Never)
    {
        for commit in [
            ws.lock.skills.get(name).and_then(|l| l.base.clone()),
            resolve_id(ctx, &id, &req, Fetch::Never).ok().map(|u| u.reference.commit),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(files) = m.list_files(&commit, &id.path) {
                paths.extend(files);
            }
        }
    }
    // Compute a candidate once rather than per file.
    let cand = if from == "candidate" || to == "candidate" { Some(candidate_dir(ctx, ws, name)?) } else { None };
    if let Some(c) = &cand {
        for e in walkdir::WalkDir::new(c.path()).into_iter().flatten() {
            if e.file_type().is_file() {
                paths.insert(e.path().strip_prefix(c.path()).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let read = |which: &str, p: &str| -> Option<Vec<u8>> {
        match (&cand, which) {
            (Some(c), "candidate") => std::fs::read(c.path().join(p)).ok(),
            _ => version_file(ctx, ws, name, which, p).ok().flatten(),
        }
    };
    let mut out = Vec::new();
    for p in paths {
        let a = read(from, &p);
        let b = read(to, &p);
        if a != b {
            out.push(p);
        }
    }
    Ok(out)
}
