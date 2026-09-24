//! Workbench installs (spec §8, §9): user-scope skills, update policies, freshness
//! without a scheduler, lock semantics and rollback.

use crate::agents::{self, Agent};
use crate::config::{self, LockedSkill, Policy, WbSkill, WorkbenchLock, WorkbenchManifest};
use crate::ctx::Ctx;
use crate::deploy::{self, PlaceRequest, Scope};
use crate::git::git;
use crate::id::{SkillId, SkillSpec, valid_skill_name};
use crate::resolve::{Fetch, ResolvedSkill, resolve_id, resolve_skill};
use crate::risk::RiskReport;
use crate::state::now;
use crate::store;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::BTreeSet;

pub fn load_manifest(ctx: &Ctx) -> Result<WorkbenchManifest> {
    WorkbenchManifest::load(&ctx.paths.workbench_manifest())
}

pub fn load_lock(ctx: &Ctx) -> Result<WorkbenchLock> {
    WorkbenchLock::load(&ctx.paths.workbench_lock())
}

pub fn installed_ids(ctx: &Ctx) -> Result<BTreeSet<String>> {
    Ok(load_lock(ctx)?.skills.into_iter().map(|s| s.id).collect())
}

pub fn default_agents(m: &WorkbenchManifest) -> Result<Vec<&'static Agent>> {
    let a = agents::parse_list(&m.settings.agents)?;
    Ok(if a.is_empty() { vec![agents::get("claude")?] } else { a })
}

fn skill_agents(m: &WorkbenchManifest, s: &WbSkill) -> Result<Vec<&'static Agent>> {
    match &s.agents {
        Some(a) if !a.is_empty() => agents::parse_list(a),
        _ => default_agents(m),
    }
}

/// Placement directory name: frontmatter name when valid, else folder name.
pub fn placement_name(name: &str, id: &SkillId) -> String {
    if valid_skill_name(name) { name.to_string() } else { id.folder_name().to_string() }
}

/// The `auto` policy is only allowed for sources owned by the user or their orgs.
pub fn ensure_trusted_for_auto(ctx: &Ctx, id: &SkillId) -> Result<()> {
    let owner = id.source.owner();
    let ident = crate::sources::cached_identity(ctx, &id.source.host);
    match ident {
        Some((login, orgs)) if owner.eq_ignore_ascii_case(&login) || orgs.iter().any(|o| o.eq_ignore_ascii_case(owner)) => Ok(()),
        Some(_) => bail!(
            "`update = \"auto\"` is only allowed for sources owned by you or your organizations; {owner} is third-party. \
             Use `update = \"unsafe-auto\"` to accept the risk explicitly."
        ),
        None => bail!("cannot verify that you own {owner} (not signed in); run `gh auth login`, or use `unsafe-auto`"),
    }
}

#[derive(Debug, Serialize)]
pub struct AddReport {
    pub id: String,
    pub canonical: String,
    pub name: String,
    pub commit: String,
    pub tree: String,
    pub agents: Vec<String>,
    pub placements: Vec<String>,
    pub license_class: Option<String>,
    pub risk: Vec<String>,
}

fn requested_from_spec(spec: &SkillSpec, r: &ResolvedSkill) -> WbSkill {
    match (&spec.reference, r.reference.kind.as_str()) {
        (None, _) => WbSkill { version: Some("latest".into()), ..Default::default() },
        (Some(x), _) if x == "latest" => WbSkill { version: Some("latest".into()), ..Default::default() },
        (Some(_), "tag") => WbSkill { version: Some(r.reference.name.clone()), ..Default::default() },
        (Some(_), "branch") => WbSkill { branch: Some(r.reference.name.clone()), ..Default::default() },
        (Some(_), _) => WbSkill { rev: Some(r.reference.commit.clone()), ..Default::default() },
    }
}

fn locked_from(r: &ResolvedSkill) -> LockedSkill {
    LockedSkill {
        id: r.id.to_string(),
        name: r.name.clone(),
        ref_kind: r.reference.kind.clone(),
        ref_name: r.reference.name.clone(),
        commit: r.reference.commit.clone(),
        tree: r.tree.clone(),
        path: None,
    }
}

/// Resolve the newest revision of an installed skill, following an upstream move of
/// its directory (detected with git rename tracking from the locked commit).
fn resolve_following(ctx: &Ctx, id: &SkillId, requested: &str, fetch: Fetch, base: &str) -> Result<(ResolvedSkill, Option<SkillId>)> {
    match resolve_id(ctx, id, requested, fetch) {
        Ok(r) if r.id != *id => {
            let moved = r.id.clone();
            Ok((r, Some(moved)))
        }
        Ok(r) => Ok((r, None)),
        Err(e) => {
            let m = crate::resolve::open_mirror(ctx, &id.source, Fetch::Never)?;
            let refs = crate::resolve::mirror_refs(&m)?;
            let Ok(target) = crate::resolve::resolve_ref(&m, &refs, Some(requested), &mut Vec::new()) else { return Err(e) };
            if m.tree_at(&target.commit, &id.path).is_some() {
                return Err(e);
            }
            let _ = m.ensure_commit(base);
            let Some(newp) = m.renamed_path(base, &target.commit, &id.path) else { return Err(e) };
            let new_id = SkillId::new(id.source.clone(), &newp);
            let r = resolve_id(ctx, &new_id, requested, Fetch::Never)?;
            Ok((r, Some(new_id)))
        }
    }
}

/// Move an installed skill to its new canonical id after an upstream rename.
fn migrate_id(ctx: &Ctx, lock: &mut WorkbenchLock, old_s: &str, old: &SkillId, new: &SkillId) -> Result<String> {
    let new_s = new.to_string();
    let path = ctx.paths.workbench_manifest();
    let mut doc = config::load_doc(&path)?;
    let t = config::table_mut(&mut doc, &["skills"]);
    if let Some(item) = t.remove(old_s) {
        t.insert(&new_s, item);
    }
    config::save_doc(&path, &doc)?;
    if let Some(mut l) = lock.get(old_s).cloned() {
        lock.remove(old_s);
        if l.path.is_none() {
            l.path = Some(old.path.clone());
        }
        l.id = new_s.clone();
        lock.upsert(l);
    }
    lock.save(&ctx.paths.workbench_lock())?;
    for table in ["placements", "pending_updates", "auto_deployed", "deployments"] {
        ctx.state.conn.execute(&format!("UPDATE {table} SET skill=?1 WHERE skill=?2"), params![new_s, old_s])?;
    }
    ctx.ui.info(&format!("upstream moved {old} → {new}; following the rename"));
    Ok(new_s)
}

pub fn add(ctx: &Ctx, input: &str, agent_names: &[String], policy: Option<Policy>, copy: bool, shadow: bool) -> Result<AddReport> {
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    if spec.source.repo_path == ".well-known/agent-skills" {
        return add_wellknown(ctx, &spec, agent_names, copy, shadow);
    }
    let r = resolve_skill(ctx, &spec, Fetch::IfStale)?;
    if let Some(p) = policy
        && p == Policy::Auto
    {
        ensure_trusted_for_auto(ctx, &r.id)?;
    }
    let m = load_manifest(ctx)?;
    let agents_sel = if agent_names.is_empty() { default_agents(&m)? } else { agents::parse_list(agent_names)? };
    let dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    let risk = RiskReport::scan_dir(&dir);
    let lic = crate::inspect::detect_license_in_dir(ctx, &r, &dir);

    let name = placement_name(&r.name, &r.id);
    let mut placements = Vec::new();
    for a in &agents_sel {
        let p = deploy::place(
            ctx,
            &PlaceRequest {
                skill: r.id.to_string(),
                origin: "workbench",
                agent: a,
                scope: Scope::Global,
                name: name.clone(),
                target: dir.clone(),
                tree: Some(r.tree.clone()),
                commit: Some(r.reference.commit.clone()),
                force_copy: copy,
                shadow,
            },
        )?;
        placements.push(format!("{} ({})", p.path, p.mode));
    }

    let mut entry = requested_from_spec(&spec, &r);
    if !agent_names.is_empty() {
        entry.agents = Some(agents_sel.iter().map(|a| a.id.to_string()).collect());
    }
    entry.update = policy;
    if copy {
        entry.mode = Some("copy".into());
    }
    let path = ctx.paths.workbench_manifest();
    let mut doc = config::load_doc(&path)?;
    config::table_mut(&mut doc, &["skills"]).insert(&r.id.to_string(), config::to_inline(&entry)?);
    config::save_doc(&path, &doc)?;
    let mut lock = load_lock(ctx)?;
    lock.upsert(locked_from(&r));
    lock.save(&ctx.paths.workbench_lock())?;
    clear_pending(ctx, &r.id.to_string())?;

    Ok(AddReport {
        id: r.id.to_string(),
        canonical: r.canonical(),
        name: r.name.clone(),
        commit: r.reference.commit.clone(),
        tree: r.tree.clone(),
        agents: agents_sel.iter().map(|a| a.id.to_string()).collect(),
        placements,
        license_class: Some(lic.class),
        risk: risk.summary(),
    })
}

fn add_wellknown(ctx: &Ctx, spec: &SkillSpec, agent_names: &[String], copy: bool, shadow: bool) -> Result<AddReport> {
    let id = SkillId::new(spec.source.clone(), &spec.selector);
    let (origin, entry) = crate::wellknown::lookup(ctx, &id.to_string())?;
    let (tmp, _url, digest) = crate::wellknown::materialize(ctx, &origin, &entry)?;
    let (dir, tree) = store::from_dir(ctx, tmp.path())?;
    let m = load_manifest(ctx)?;
    let agents_sel = if agent_names.is_empty() { default_agents(&m)? } else { agents::parse_list(agent_names)? };
    let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
    let name = placement_name(&crate::skill::SkillDoc::parse(&text).name.unwrap_or_default(), &id);
    let mut placements = Vec::new();
    for a in &agents_sel {
        let p = deploy::place(
            ctx,
            &PlaceRequest {
                skill: id.to_string(),
                origin: "workbench",
                agent: a,
                scope: Scope::Global,
                name: name.clone(),
                target: dir.clone(),
                tree: Some(tree.clone()),
                commit: Some(digest.clone()),
                force_copy: copy,
                shadow,
            },
        )?;
        placements.push(format!("{} ({})", p.path, p.mode));
    }
    let path = ctx.paths.workbench_manifest();
    let mut doc = config::load_doc(&path)?;
    let mut wb = WbSkill { version: Some("latest".into()), ..Default::default() };
    if !agent_names.is_empty() {
        wb.agents = Some(agents_sel.iter().map(|a| a.id.to_string()).collect());
    }
    config::table_mut(&mut doc, &["skills"]).insert(&id.to_string(), config::to_inline(&wb)?);
    config::save_doc(&path, &doc)?;
    let mut lock = load_lock(ctx)?;
    lock.upsert(wellknown_locked(&id, &name, &digest, &tree));
    lock.save(&ctx.paths.workbench_lock())?;
    Ok(AddReport {
        id: id.to_string(),
        canonical: id.to_string(),
        name,
        commit: digest,
        tree,
        agents: agents_sel.iter().map(|a| a.id.to_string()).collect(),
        placements,
        license_class: Some(crate::license::detect(&crate::inspect::gather_dir(&dir)).class),
        risk: RiskReport::scan_dir(&dir).summary(),
    })
}

fn wellknown_locked(id: &SkillId, name: &str, digest: &str, tree: &str) -> LockedSkill {
    let short = digest.trim_start_matches("sha256:");
    LockedSkill {
        id: id.to_string(),
        name: name.to_string(),
        ref_kind: "digest".into(),
        ref_name: short[..12.min(short.len())].to_string(),
        commit: digest.to_string(),
        tree: tree.to_string(),
        path: None,
    }
}

/// A newer published version of a well-known skill (by digest), after refreshing its site's index.
fn wellknown_newer(ctx: &Ctx, locked: &LockedSkill, fetch: bool) -> Result<Option<(String, crate::wellknown::Entry)>> {
    let (origin, _) = crate::wellknown::lookup(ctx, &locked.id)?;
    if fetch {
        let _ = crate::sources::refresh(ctx, false, Some(&origin));
    }
    let (origin, entry) = crate::wellknown::lookup(ctx, &locked.id)?;
    Ok(entry.digest.clone().filter(|d| d != &locked.commit).map(|_| (origin, entry)))
}

/// Find an installed skill by canonical id, short id or name.
pub fn find_installed(ctx: &Ctx, input: &str) -> Result<(String, WbSkill)> {
    let m = load_manifest(ctx)?;
    if let Some(s) = m.skills.get(input) {
        return Ok((input.to_string(), s.clone()));
    }
    if input.contains("//")
        && let Ok(spec) = SkillSpec::parse(input)
    {
        let lock = load_lock(ctx)?;
        for (id, s) in &m.skills {
            let Ok(sid) = SkillId::parse_canonical(id) else { continue };
            if sid.source == spec.source {
                let name = lock.get(id).map(|l| l.name.clone()).unwrap_or_default();
                if sid.path == spec.selector || name == spec.selector || sid.folder_name() == spec.selector {
                    return Ok((id.clone(), s.clone()));
                }
            }
        }
    }
    let lock = load_lock(ctx)?;
    let hits: Vec<&LockedSkill> = lock.skills.iter().filter(|l| l.name == input || l.id.rsplit('/').next() == Some(input)).collect();
    match hits.len() {
        1 => {
            let id = hits[0].id.clone();
            let s = m.skills.get(&id).cloned().unwrap_or_default();
            Ok((id, s))
        }
        0 => bail!("`{input}` is not installed in the workbench"),
        _ => bail!("`{input}` matches several installed skills: {}", hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>().join(", ")),
    }
}

pub fn remove(ctx: &Ctx, input: &str) -> Result<Vec<String>> {
    let (id, _) = find_installed(ctx, input)?;
    let mut removed = Vec::new();
    for p in ctx.state.placements("WHERE skill=?1 AND origin='workbench'", &[&id])? {
        deploy::remove_placement(ctx, &p)?;
        removed.push(p.path);
    }
    let path = ctx.paths.workbench_manifest();
    let mut doc = config::load_doc(&path)?;
    config::table_mut(&mut doc, &["skills"]).remove(&id);
    config::save_doc(&path, &doc)?;
    let mut lock = load_lock(ctx)?;
    lock.remove(&id);
    lock.save(&ctx.paths.workbench_lock())?;
    clear_pending(ctx, &id)?;
    ctx.state.conn.execute("DELETE FROM auto_deployed WHERE skill=?1", [&id])?;
    Ok(removed)
}

fn clear_pending(ctx: &Ctx, id: &str) -> Result<()> {
    ctx.state.conn.execute("DELETE FROM pending_updates WHERE skill=?1", [id])?;
    Ok(())
}

/// Ensure placements for `id` match `agents` at user scope, pointing at `dir`.
#[allow(clippy::too_many_arguments)]
fn sync_placements(
    ctx: &Ctx,
    id: &str,
    name: &str,
    agents_sel: &[&'static Agent],
    dir: &std::path::Path,
    tree: &str,
    commit: &str,
    copy: bool,
) -> Result<usize> {
    let existing = ctx.state.placements("WHERE skill=?1 AND origin='workbench'", &[&id])?;
    let mut changed = 0;
    for p in &existing {
        if !agents_sel.iter().any(|a| a.id == p.agent) {
            deploy::remove_placement(ctx, p)?;
            changed += 1;
        }
    }
    for a in agents_sel {
        let cur = existing.iter().find(|p| p.agent == a.id);
        let up_to_date = cur.map(|p| p.tree.as_deref() == Some(tree) && deploy::health(p) == "ok").unwrap_or(false);
        if up_to_date {
            continue;
        }
        deploy::place(
            ctx,
            &PlaceRequest {
                skill: id.to_string(),
                origin: "workbench",
                agent: a,
                scope: Scope::Global,
                name: name.to_string(),
                target: dir.to_path_buf(),
                tree: Some(tree.to_string()),
                commit: Some(commit.to_string()),
                force_copy: copy,
                shadow: false,
            },
        )?;
        changed += 1;
    }
    Ok(changed)
}

#[derive(Debug, Serialize)]
pub struct InstallItem {
    pub id: String,
    pub name: String,
    /// ok | placed | auto-updated | update-ready | error
    pub action: String,
    pub commit: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Serialize, Default)]
pub struct InstallReport {
    pub skills: Vec<InstallItem>,
    pub updates_ready: usize,
}

pub fn install(ctx: &Ctx, frozen: bool) -> Result<InstallReport> {
    let m = load_manifest(ctx)?;
    let mut lock = load_lock(ctx)?;
    let mut rep = InstallReport::default();
    let journal = ctx.state.journal_start("install", if frozen { "frozen" } else { "" })?;
    for (id_s, entry) in &m.skills {
        let item = install_one(ctx, &m, &mut lock, id_s, entry, frozen);
        match item {
            Ok(it) => {
                if it.action == "update-ready" {
                    rep.updates_ready += 1;
                }
                rep.skills.push(it);
            }
            Err(e) => rep.skills.push(InstallItem {
                id: id_s.clone(),
                name: String::new(),
                action: "error".into(),
                commit: None,
                detail: Some(format!("{e:#}")),
            }),
        }
    }
    lock.save(&ctx.paths.workbench_lock())?;
    if let Ok(n) = crate::agentskill::refresh_if_outdated(ctx)
        && n > 0
    {
        ctx.ui.info("refreshed the bundled agent skill");
    }
    ctx.state.journal_finish(journal, "done")?;
    Ok(rep)
}

fn install_one(
    ctx: &Ctx,
    m: &WorkbenchManifest,
    lock: &mut WorkbenchLock,
    id_s: &str,
    entry: &WbSkill,
    frozen: bool,
) -> Result<InstallItem> {
    let id = SkillId::parse_canonical(id_s)?;
    if id.source.repo_path == ".well-known/agent-skills" {
        let l = lock.get(id_s).cloned().context("well-known skill missing from lock; re-add it")?;
        let dir = store::entry_path(ctx, &l.tree);
        if !dir.exists() {
            bail!("store entry for {id_s} missing; re-add it");
        }
        let agents_sel = skill_agents(m, entry)?;
        let n = sync_placements(
            ctx,
            id_s,
            &placement_name(&l.name, &id),
            &agents_sel,
            &dir,
            &l.tree,
            &l.commit,
            entry.mode.as_deref() == Some("copy"),
        )?;
        if !frozen
            && !matches!(entry.policy(), Policy::Paused | Policy::Pinned)
            && let Ok(Some((_, e))) = wellknown_newer(ctx, &l, true)
        {
            let d = e.digest.unwrap_or_default();
            ctx.state.conn.execute(
                "INSERT OR REPLACE INTO pending_updates(skill, from_commit, to_commit, to_tree, ref_kind, ref_name, risk, at) VALUES(?1,?2,?3,NULL,'digest',?4,'',?5)",
                params![id_s, l.commit, d, d.trim_start_matches("sha256:").chars().take(12).collect::<String>(), now()],
            )?;
            return Ok(InstallItem {
                id: id_s.into(),
                name: l.name,
                action: "update-ready".into(),
                commit: Some(l.commit),
                detail: Some("new digest published".into()),
            });
        }
        return Ok(InstallItem {
            id: id_s.into(),
            name: l.name,
            action: if n > 0 { "placed" } else { "ok" }.into(),
            commit: Some(l.commit),
            detail: None,
        });
    }
    let policy = entry.policy();
    let fetch = if frozen || policy == Policy::Paused || policy == Policy::Pinned { Fetch::Never } else { Fetch::IfStale };
    let locked = match lock.get(id_s).cloned() {
        Some(l) => l,
        None => {
            if frozen {
                bail!("{id_s} is not in tricks.lock (--frozen)");
            }
            let r = resolve_id(ctx, &id, &entry.requested_ref(), Fetch::IfStale)?;
            let l = locked_from(&r);
            lock.upsert(l.clone());
            l
        }
    };
    let mirror = crate::resolve::open_mirror(ctx, &id.source, fetch)?;
    mirror.ensure_commit(&locked.commit)?;
    let mut deploy_commit = locked.commit.clone();
    let mut deploy_tree = locked.tree.clone();
    let mut action = "ok".to_string();
    let mut detail = None;

    if !frozen && policy != Policy::Paused && policy != Policy::Pinned {
        let (newest, moved) = resolve_following(ctx, &id, &entry.requested_ref(), Fetch::Never, &locked.commit)?;
        if let Some(new_id) = moved {
            let new_s = migrate_id(ctx, lock, id_s, &id, &new_id)?;
            let m2 = load_manifest(ctx)?;
            let entry2 = m2.skills.get(&new_s).cloned().unwrap_or_default();
            return install_one(ctx, &m2, lock, &new_s, &entry2, frozen);
        }
        if newest.reference.commit != locked.commit && newest.tree != locked.tree {
            if policy.is_auto() {
                if policy == Policy::Auto {
                    ensure_trusted_for_auto(ctx, &id)?;
                }
                deploy_commit = newest.reference.commit.clone();
                deploy_tree = newest.tree.clone();
                ctx.state.conn.execute(
                    "INSERT OR REPLACE INTO auto_deployed(skill, commit_sha, tree, at) VALUES(?1,?2,?3,?4)",
                    params![id_s, deploy_commit, deploy_tree, now()],
                )?;
                action = "auto-updated".into();
                detail = Some(format!("{} → {} (lock unchanged)", short(&locked.commit), short(&deploy_commit)));
            } else {
                let risk = update_risk(ctx, &newest, &locked).unwrap_or_default();
                ctx.state.conn.execute(
                    "INSERT OR REPLACE INTO pending_updates(skill, from_commit, to_commit, to_tree, ref_kind, ref_name, risk, at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![id_s, locked.commit, newest.reference.commit, newest.tree, newest.reference.kind, newest.reference.name, risk.join("\n"), now()],
                )?;
                action = "update-ready".into();
                detail = Some(format!("{} → {}", locked.ref_name, newest.reference.name));
            }
        } else if let Some(auto) = auto_deployed(ctx, id_s)?
            && policy.is_auto()
        {
            deploy_commit = auto.0;
            deploy_tree = auto.1;
        }
    }
    let export_path = if deploy_commit == locked.commit { locked.path.clone().unwrap_or(id.path.clone()) } else { id.path.clone() };
    let dir = store::from_mirror(ctx, &mirror, &deploy_commit, &export_path, &deploy_tree)?;
    let agents_sel = skill_agents(m, entry)?;
    let n = sync_placements(
        ctx,
        id_s,
        &placement_name(&locked.name, &id),
        &agents_sel,
        &dir,
        &deploy_tree,
        &deploy_commit,
        entry.mode.as_deref() == Some("copy"),
    )?;
    if n > 0 && action == "ok" {
        action = "placed".into();
    }
    Ok(InstallItem { id: id_s.into(), name: locked.name.clone(), action, commit: Some(deploy_commit), detail })
}

fn auto_deployed(ctx: &Ctx, id: &str) -> Result<Option<(String, String)>> {
    Ok(ctx
        .state
        .conn
        .query_row("SELECT commit_sha, tree FROM auto_deployed WHERE skill=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?)
}

fn short(c: &str) -> &str {
    &c[..c.len().min(9)]
}

/// Risk and file changes between the locked revision and a candidate.
fn update_risk(ctx: &Ctx, newest: &ResolvedSkill, locked: &LockedSkill) -> Result<Vec<String>> {
    let new_dir = store::from_mirror(ctx, &newest.mirror, &newest.reference.commit, &newest.id.path, &newest.tree)?;
    let old_path = locked.path.clone().unwrap_or(newest.id.path.clone());
    let old_dir = store::from_mirror(ctx, &newest.mirror, &locked.commit, &old_path, &locked.tree)?;
    let mut lines = file_changes(&newest.mirror.dir, &locked.commit, &newest.reference.commit, &newest.id.path);
    lines.extend(RiskReport::scan_dir(&new_dir).diff_from(&RiskReport::scan_dir(&old_dir)).into_iter().map(|l| format!("risk: {l}")));
    Ok(lines)
}

pub fn file_changes(repo: &std::path::Path, from: &str, to: &str, path: &str) -> Vec<String> {
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

#[derive(Debug, Serialize)]
pub struct UpdateItem {
    pub id: String,
    pub name: String,
    pub from: String,
    pub to: String,
    pub from_ref: String,
    pub to_ref: String,
    pub changes: Vec<String>,
    pub applied: bool,
}

pub fn update(ctx: &Ctx, only: Option<&str>) -> Result<Vec<UpdateItem>> {
    let m = load_manifest(ctx)?;
    let mut lock = load_lock(ctx)?;
    let targets: Vec<String> = match only {
        Some(s) => vec![find_installed(ctx, s)?.0],
        None => m.skills.keys().cloned().collect(),
    };
    let mut out = Vec::new();
    for id_s in targets {
        let entry = m.skills.get(&id_s).cloned().unwrap_or_default();
        let mut id_s = id_s;
        let mut id = SkillId::parse_canonical(&id_s)?;
        if only.is_none() && matches!(entry.policy(), Policy::Pinned | Policy::Paused) {
            continue;
        }
        if id.source.repo_path == ".well-known/agent-skills" {
            let Some(locked) = lock.get(&id_s).cloned() else { continue };
            let Some((origin, e)) = wellknown_newer(ctx, &locked, true)? else { continue };
            let (tmp, _url, digest) = crate::wellknown::materialize(ctx, &origin, &e)?;
            let (dir, tree) = store::from_dir(ctx, tmp.path())?;
            let old_dir = store::entry_path(ctx, &locked.tree);
            let changes: Vec<String> =
                RiskReport::scan_dir(&dir).diff_from(&RiskReport::scan_dir(&old_dir)).into_iter().map(|l| format!("risk: {l}")).collect();
            let to_ref = digest.trim_start_matches("sha256:").chars().take(12).collect::<String>();
            let mut details = vec![format!("{}: digest {} → {to_ref}", locked.name, locked.ref_name)];
            details.extend(changes.iter().map(|c| format!("  {c}")));
            let applied = ctx.confirm(&format!("Apply update to {}?", locked.name), &details)?;
            if applied {
                let agents_sel = skill_agents(&m, &entry)?;
                sync_placements(
                    ctx,
                    &id_s,
                    &placement_name(&locked.name, &id),
                    &agents_sel,
                    &dir,
                    &tree,
                    &digest,
                    entry.mode.as_deref() == Some("copy"),
                )?;
                lock.upsert(wellknown_locked(&id, &locked.name, &digest, &tree));
                clear_pending(ctx, &id_s)?;
            }
            out.push(UpdateItem {
                id: id_s.clone(),
                name: locked.name.clone(),
                from: locked.commit.clone(),
                to: digest.clone(),
                from_ref: locked.ref_name.clone(),
                to_ref,
                changes,
                applied,
            });
            continue;
        }
        let Some(locked) = lock.get(&id_s).cloned() else { continue };
        let (newest, moved) = resolve_following(ctx, &id, &entry.requested_ref(), Fetch::IfStale, &locked.commit)?;
        if let Some(new_id) = moved {
            id_s = migrate_id(ctx, &mut lock, &id_s, &id, &new_id)?;
            id = new_id;
        }
        let Some(locked) = lock.get(&id_s).cloned() else { continue };
        if newest.reference.commit == locked.commit || newest.tree == locked.tree {
            if newest.reference.commit != locked.commit {
                lock.upsert(locked_from(&newest)); // same content, newer commit
            }
            continue;
        }
        let changes = update_risk(ctx, &newest, &locked)?;
        let mut details = vec![format!("{}: {} → {}", locked.name, locked.ref_name, newest.reference.name)];
        details.extend(changes.iter().map(|c| format!("  {c}")));
        let applied = ctx.confirm(&format!("Apply update to {}?", locked.name), &details)?;
        if applied {
            let dir = store::from_mirror(ctx, &newest.mirror, &newest.reference.commit, &id.path, &newest.tree)?;
            let agents_sel = skill_agents(&m, &entry)?;
            sync_placements(
                ctx,
                &id_s,
                &placement_name(&newest.name, &id),
                &agents_sel,
                &dir,
                &newest.tree,
                &newest.reference.commit,
                entry.mode.as_deref() == Some("copy"),
            )?;
            lock.upsert(locked_from(&newest));
            clear_pending(ctx, &id_s)?;
            ctx.state.conn.execute("DELETE FROM auto_deployed WHERE skill=?1", [&id_s])?;
        }
        out.push(UpdateItem {
            id: id_s.clone(),
            name: locked.name.clone(),
            from: locked.commit.clone(),
            to: newest.reference.commit.clone(),
            from_ref: locked.ref_name.clone(),
            to_ref: newest.reference.name.clone(),
            changes,
            applied,
        });
    }
    lock.save(&ctx.paths.workbench_lock())?;
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct Pending {
    pub id: String,
    pub name: String,
    pub from_ref: String,
    pub to_ref: String,
    pub to_commit: String,
    pub policy: String,
    pub details: Vec<String>,
}

/// Skills with a newer revision available (fetching sources when due).
pub fn outdated(ctx: &Ctx) -> Result<Vec<Pending>> {
    let m = load_manifest(ctx)?;
    let lock = load_lock(ctx)?;
    let mut out = Vec::new();
    for (id_s, entry) in &m.skills {
        let policy = entry.policy();
        if policy == Policy::Paused {
            continue;
        }
        let Ok(id) = SkillId::parse_canonical(id_s) else { continue };
        let Some(locked) = lock.get(id_s) else { continue };
        if id.source.repo_path == ".well-known/agent-skills" {
            if let Ok(Some((_, e))) = wellknown_newer(ctx, locked, true) {
                let d = e.digest.unwrap_or_default();
                out.push(Pending {
                    id: id_s.clone(),
                    name: locked.name.clone(),
                    from_ref: locked.ref_name.clone(),
                    to_ref: d.trim_start_matches("sha256:").chars().take(12).collect(),
                    to_commit: d,
                    policy: policy.as_str().into(),
                    details: vec!["new digest published in the site's index".into()],
                });
            }
            continue;
        }
        let (newest, moved) = match resolve_following(ctx, &id, &entry.requested_ref(), Fetch::IfStale, &locked.commit) {
            Ok(n) => n,
            Err(e) => {
                ctx.ui.warn(&format!("{id_s}: {e:#}"));
                continue;
            }
        };
        if moved.is_some() || (newest.reference.commit != locked.commit && newest.tree != locked.tree) {
            let mut details = update_risk(ctx, &newest, locked).unwrap_or_default();
            if let Some(n) = &moved {
                details.insert(0, format!("moved upstream: {} → {} (followed on update)", id.path, n.path));
            }
            if !policy.is_auto() {
                ctx.state.conn.execute(
                    "INSERT OR REPLACE INTO pending_updates(skill, from_commit, to_commit, to_tree, ref_kind, ref_name, risk, at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![id_s, locked.commit, newest.reference.commit, newest.tree, newest.reference.kind, newest.reference.name, details.join("\n"), now()],
                )?;
            }
            out.push(Pending {
                id: id_s.clone(),
                name: locked.name.clone(),
                from_ref: locked.ref_name.clone(),
                to_ref: newest.reference.name.clone(),
                to_commit: newest.reference.commit.clone(),
                policy: policy.as_str().into(),
                details,
            });
        }
    }
    Ok(out)
}

#[derive(Debug, Serialize)]
pub struct PlacementInfo {
    pub skill: String,
    pub origin: String,
    pub agent: String,
    pub scope: String,
    pub path: String,
    pub mode: String,
    pub commit: Option<String>,
    pub health: String,
}

#[derive(Debug, Serialize)]
pub struct SkillStatus {
    pub id: String,
    pub name: String,
    pub ref_kind: String,
    pub ref_name: String,
    pub commit: String,
    pub policy: String,
    pub deployed_commit: Option<String>,
    pub ahead_of_lock: bool,
    pub pending: Option<String>,
    pub placements: Vec<PlacementInfo>,
}

#[derive(Debug, Serialize)]
pub struct StatusReport {
    pub skills: Vec<SkillStatus>,
    pub links: Vec<PlacementInfo>,
    pub updates_ready: usize,
    pub unfinished_operations: Vec<String>,
}

fn info(p: &crate::state::Placement) -> PlacementInfo {
    PlacementInfo {
        skill: p.skill.clone(),
        origin: p.origin.clone(),
        agent: p.agent.clone(),
        scope: p.scope.clone(),
        path: p.path.clone(),
        mode: p.mode.clone(),
        commit: p.commit.clone(),
        health: deploy::health(p),
    }
}

pub fn status(ctx: &Ctx) -> Result<StatusReport> {
    let m = load_manifest(ctx)?;
    let lock = load_lock(ctx)?;
    let mut skills = Vec::new();
    let mut updates_ready = 0;
    for (id_s, entry) in &m.skills {
        let locked = lock.get(id_s);
        let placements: Vec<PlacementInfo> =
            ctx.state.placements("WHERE skill=?1 AND origin='workbench'", &[id_s])?.iter().map(info).collect();
        let deployed = placements.first().and_then(|p| p.commit.clone());
        let pending: Option<String> =
            ctx.state.conn.query_row("SELECT ref_name FROM pending_updates WHERE skill=?1", [id_s], |r| r.get(0)).optional()?;
        if pending.is_some() {
            updates_ready += 1;
        }
        skills.push(SkillStatus {
            id: id_s.clone(),
            name: locked.map(|l| l.name.clone()).unwrap_or_default(),
            ref_kind: locked.map(|l| l.ref_kind.clone()).unwrap_or_default(),
            ref_name: locked.map(|l| l.ref_name.clone()).unwrap_or_default(),
            commit: locked.map(|l| l.commit.clone()).unwrap_or_default(),
            policy: entry.policy().as_str().into(),
            ahead_of_lock: match (locked, &deployed) {
                (Some(l), Some(d)) => &l.commit != d,
                _ => false,
            },
            deployed_commit: deployed,
            pending,
            placements,
        });
    }
    let links = ctx.state.placements("WHERE origin<>'workbench'", &[])?.iter().map(info).collect();
    let unfinished = ctx.state.unfinished_ops()?.into_iter().map(|(id, op, d)| format!("#{id} {op} {d}")).collect();
    Ok(StatusReport { skills, links, updates_ready, unfinished_operations: unfinished })
}

#[derive(Debug, Serialize)]
pub struct RollbackReport {
    pub id: String,
    pub from: String,
    pub to: String,
}

/// Roll back to the previously deployed revision and pin it so the next check does
/// not undo the rollback.
pub fn rollback(ctx: &Ctx, input: &str) -> Result<RollbackReport> {
    let (id_s, entry) = find_installed(ctx, input)?;
    let mut lock = load_lock(ctx)?;
    let locked = lock.get(&id_s).cloned().context("not in lock")?;
    let current_tree = ctx
        .state
        .placements("WHERE skill=?1 AND origin='workbench'", &[&id_s])?
        .first()
        .and_then(|p| p.tree.clone())
        .unwrap_or(locked.tree.clone());
    let mut st = ctx.state.conn.prepare(
        "SELECT tree, commit_sha FROM deployments WHERE skill=?1 AND action='place' AND tree IS NOT NULL AND tree<>?2 ORDER BY at DESC, id DESC LIMIT 1",
    )?;
    let prev: Option<(String, String)> = st.query_row(params![id_s, current_tree], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let (tree, commit) = prev.context("no previous revision recorded for this skill")?;
    let id = SkillId::parse_canonical(&id_s)?;
    let dir = store::entry_path(ctx, &tree);
    let dir = if dir.exists() {
        dir
    } else {
        let mirror = crate::resolve::open_mirror(ctx, &id.source, Fetch::Never)?;
        mirror.ensure_commit(&commit)?;
        store::from_mirror(ctx, &mirror, &commit, &id.path, &tree)?
    };
    let m = load_manifest(ctx)?;
    let agents_sel = skill_agents(&m, &entry)?;
    sync_placements(
        ctx,
        &id_s,
        &placement_name(&locked.name, &id),
        &agents_sel,
        &dir,
        &tree,
        &commit,
        entry.mode.as_deref() == Some("copy"),
    )?;
    lock.upsert(LockedSkill {
        ref_kind: "commit".into(),
        ref_name: commit[..12.min(commit.len())].to_string(),
        commit: commit.clone(),
        tree,
        ..locked.clone()
    });
    lock.save(&ctx.paths.workbench_lock())?;
    let path = ctx.paths.workbench_manifest();
    let mut doc = config::load_doc(&path)?;
    let mut e = entry.clone();
    e.update = Some(Policy::Pinned);
    config::table_mut(&mut doc, &["skills"]).insert(&id_s, config::to_inline(&e)?);
    config::save_doc(&path, &doc)?;
    ctx.state.conn.execute("DELETE FROM auto_deployed WHERE skill=?1", [&id_s])?;
    clear_pending(ctx, &id_s)?;
    Ok(RollbackReport { id: id_s, from: locked.commit, to: commit })
}

/// Set the update policy of an installed skill.
pub fn set_policy(ctx: &Ctx, input: &str, policy: Policy) -> Result<String> {
    let (id_s, mut entry) = find_installed(ctx, input)?;
    if policy == Policy::Auto {
        ensure_trusted_for_auto(ctx, &SkillId::parse_canonical(&id_s)?)?;
    }
    entry.update = Some(policy);
    let path = ctx.paths.workbench_manifest();
    let mut doc = config::load_doc(&path)?;
    config::table_mut(&mut doc, &["skills"]).insert(&id_s, config::to_inline(&entry)?);
    config::save_doc(&path, &doc)?;
    Ok(id_s)
}
