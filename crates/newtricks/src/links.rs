//! Links (spec §8): `link` deploys source repo skills and `try` deploys anything else
//! (upstream skills, local folders) into a project or the user-level agent directories,
//! to test them with real agents.

use crate::agents::{self, Agent};
use crate::ctx::Ctx;
use crate::deploy::{self, PlaceRequest, Scope};
use crate::id::SkillId;
use crate::resolve::{Fetch, resolve_skill};
use crate::skill::SkillDoc;
use crate::store;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// What a link points at.
pub struct LinkTarget {
    pub skill: String,
    pub name: String,
    pub dir: PathBuf,
    pub tree: Option<String>,
    pub commit: Option<String>,
    pub dev: bool,
    /// An upstream skill linked without vendoring it.
    pub trial: bool,
    /// The branch a source repo skill's link is pinned to (`link <skill>@<branch>`).
    pub pin: Option<String>,
    /// The branch it deploys.
    pub branch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Linked {
    pub skill: String,
    pub name: String,
    pub scope: String,
    pub trial: bool,
    /// The branch a source repo skill's link deploys, and whether it is pinned to it.
    pub branch: Option<String>,
    pub pinned: bool,
    /// working-tree | draft | snapshot (source repo skills)
    pub source: Option<String>,
    pub commit: Option<String>,
    /// (agent, path, mode)
    pub placements: Vec<(String, String, String)>,
}

#[derive(Debug, Serialize, Default)]
pub struct LinkReport {
    pub links: Vec<Linked>,
    /// Skills that could not be linked, with the reason.
    pub errors: Vec<(String, String)>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LinkOptions<'a> {
    pub to: Option<&'a str>,
    pub global: bool,
    pub agents: &'a [String],
    pub copy: bool,
    pub shadow: bool,
}

/// Target scope: `--to <dir>`, `--global`, or the default — user-level agent directories
/// for source repo and local skills, the current project for upstream trials.
fn scope_for(ctx: &Ctx, o: &LinkOptions, trial: bool) -> Result<Scope> {
    match (o.to, o.global) {
        (Some(_), true) => bail!("use either --to <path> or --global"),
        (Some(p), false) => {
            let p = ctx.paths.expand(p);
            let p = if p.is_absolute() { p } else { ctx.opts.cwd.join(p) };
            let p = crate::paths::canon(&p).with_context(|| format!("target {} does not exist", p.display()))?;
            if !p.is_dir() {
                bail!("target {} is not a directory", p.display());
            }
            Ok(Scope::Project(p))
        }
        (None, true) => Ok(Scope::Global),
        (None, false) if trial => Ok(Scope::Project(crate::paths::canon(&ctx.opts.cwd)?)),
        (None, false) => Ok(Scope::Global),
    }
}

fn agents_for(ctx: &Ctx, names: &[String]) -> Result<Vec<&'static Agent>> {
    if !names.is_empty() {
        return agents::parse_list(names);
    }
    match crate::source_repo::current(ctx)? {
        Some(ws) => ws.agents(ctx),
        None => crate::user::default_agents(&crate::user::config(ctx)?),
    }
}

/// A skill of the current source repo: a name, `name@branch`, or a directory inside it.
fn repo_target(ctx: &Ctx, input: &str) -> Result<Option<LinkTarget>> {
    if let Some(t) = crate::source_repo::link_target_for_name(ctx, input)? {
        return Ok(Some(t));
    }
    if let Some(dir) = local_dir(ctx, input)
        && let Some(key) = crate::source_repo::skill_key_for_dir(&dir)
    {
        let name = key.rsplit_once("//").map(|(_, n)| n.to_string()).unwrap_or_default();
        return crate::source_repo::link_target_for_name(ctx, &name);
    }
    Ok(None)
}

fn local_dir(ctx: &Ctx, input: &str) -> Option<PathBuf> {
    let p = Path::new(input);
    let local = if p.is_absolute() { p.to_path_buf() } else { ctx.opts.cwd.join(p) };
    let pathlike = input.starts_with('.') || input.starts_with('/') || input.contains(std::path::MAIN_SEPARATOR) || !input.contains("//");
    (pathlike && local.join("SKILL.md").is_file()).then(|| crate::paths::canon(&local).ok()).flatten()
}

/// Something to try that is not in the source repo: a local skill folder (dev mode) or
/// an upstream skill (an exact revision in the store).
fn trial_target(ctx: &Ctx, input: &str) -> Result<LinkTarget> {
    if let Some(dir) = local_dir(ctx, input) {
        let doc = SkillDoc::parse(&std::fs::read_to_string(dir.join("SKILL.md"))?);
        let folder = dir.file_name().unwrap().to_string_lossy().to_string();
        let name = doc.name.filter(|n| crate::id::valid_skill_name(n)).unwrap_or(folder);
        return Ok(LinkTarget {
            skill: format!("local:{}", dir.display()),
            name,
            dir,
            tree: None,
            commit: None,
            dev: true,
            trial: true,
            pin: None,
            branch: None,
        });
    }
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    let hosted = SkillId::new(spec.source.clone(), &spec.selector);
    if crate::hosted::kind(&hosted).is_some() {
        return match crate::hosted::fetch_to_store(ctx, &hosted, spec.reference.as_deref().filter(|r| *r != "latest"))? {
            crate::hosted::StoreOutcome::Stored(st) => {
                let doc = SkillDoc::parse(&std::fs::read_to_string(st.dir.join("SKILL.md")).unwrap_or_default());
                Ok(LinkTarget {
                    skill: hosted.to_string(),
                    name: crate::user::placement_name(&doc.name.unwrap_or_default(), &hosted),
                    dir: st.dir,
                    tree: Some(st.tree),
                    commit: Some(st.commit),
                    dev: false,
                    trial: true,
                    pin: None,
                    branch: None,
                })
            }
            crate::hosted::StoreOutcome::Redirect(git_id) => trial_target(ctx, &git_id),
        };
    }
    let r = resolve_skill(ctx, &spec, Fetch::IfStale)?;
    let dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    Ok(LinkTarget {
        skill: r.id.to_string(),
        name: crate::user::placement_name(&r.name, &r.id),
        dir,
        tree: Some(r.tree.clone()),
        commit: Some(r.reference.commit.clone()),
        dev: false,
        trial: true,
        pin: None,
        branch: None,
    })
}

/// Placement key a skill name or id refers to (for `unlink`).
pub fn target_key(ctx: &Ctx, input: &str) -> Option<String> {
    match repo_target(ctx, input) {
        Ok(Some(t)) => Some(t.skill),
        _ => trial_target(ctx, input).ok().map(|t| t.skill),
    }
}

/// `tricks link [skill]`: link a source repo skill, or with no skill all of them.
pub fn link(ctx: &Ctx, input: Option<&str>, o: &LinkOptions) -> Result<LinkReport> {
    let agents_sel = agents_for(ctx, o.agents)?;
    let mut rep = LinkReport::default();
    let Some(input) = input else {
        let ws = crate::source_repo::current(ctx)?
            .context("`tricks link` links the skills of a source repo: run it inside one, or use `tricks try <skill>` for anything else")?;
        let scope = scope_for(ctx, o, false)?;
        for name in ws.manifest.skills.keys() {
            match crate::source_repo::place_skill(ctx, &ws, name, &agents_sel, &scope, o.copy, o.shadow) {
                Ok(ps) => rep.links.push(Linked {
                    skill: ws.skill_key(name),
                    name: name.clone(),
                    scope: scope.key(),
                    trial: false,
                    branch: ps.first().and_then(|p| p.branch.clone()),
                    pinned: ps.iter().any(|p| p.pin.is_some()),
                    source: ps.first().map(|p| source_kind(&p.target, p.commit.as_deref()).to_string()),
                    commit: ps.first().and_then(|p| p.commit.clone()),
                    placements: ps.into_iter().map(|p| (p.agent, p.path, p.mode)).collect(),
                }),
                Err(e) => rep.errors.push((name.clone(), format!("{e:#}"))),
            }
        }
        return Ok(rep);
    };
    let Some(t) = repo_target(ctx, input)? else {
        match crate::source_repo::current(ctx)? {
            Some(ws) => {
                bail!("`{input}` is not a skill in source repo {}; to try a skill from elsewhere, use `tricks try {input}`", ws.name)
            }
            None => bail!("not inside a source repo; to try `{input}`, use `tricks try {input}`"),
        }
    };
    rep.links.push(place(ctx, t, o, &agents_sel)?);
    Ok(rep)
}

/// `tricks try <skill>`: link a skill that is not in the source repo, to evaluate it.
pub fn try_skill(ctx: &Ctx, input: &str, o: &LinkOptions) -> Result<LinkReport> {
    if repo_target(ctx, input)?.is_some() {
        bail!("`{input}` is a skill of this source repo; link it with `tricks link {input}`");
    }
    let agents_sel = agents_for(ctx, o.agents)?;
    let t = trial_target(ctx, input)?;
    Ok(LinkReport { links: vec![place(ctx, t, o, &agents_sel)?], errors: vec![] })
}

fn place(ctx: &Ctx, t: LinkTarget, o: &LinkOptions, agents_sel: &[&'static Agent]) -> Result<Linked> {
    let scope = scope_for(ctx, o, t.trial)?;
    if t.dev && agents_sel.iter().any(|a| !a.follows_links(&t.dir)) && !o.copy {
        ctx.ui.warn("some selected agents cannot follow links here; they get a copy, so live edits will not show until you re-link");
    }
    let origin = if t.trial { "trial" } else { "source-repo" };
    let mut placements = Vec::new();
    for a in agents_sel {
        let p = deploy::place(
            ctx,
            &PlaceRequest {
                skill: t.skill.clone(),
                origin,
                agent: a,
                scope: scope.clone(),
                name: t.name.clone(),
                target: t.dir.clone(),
                tree: t.tree.clone(),
                commit: t.commit.clone(),
                force_copy: o.copy,
                shadow: o.shadow,
                pin: t.pin.clone(),
                branch: t.branch.clone(),
            },
        )?;
        placements.push((a.id.to_string(), p.path, p.mode));
    }
    let source = (!t.trial).then(|| source_kind(&t.dir.to_string_lossy(), t.commit.as_deref()).to_string());
    Ok(Linked {
        skill: t.skill,
        name: t.name,
        scope: scope.key(),
        trial: t.trial,
        branch: t.branch,
        pinned: t.pin.is_some(),
        source,
        commit: t.commit,
        placements,
    })
}

/// Where a source repo skill's link points: `working-tree`, `draft` (an experiment
/// checked out by `tricks edit`) or `snapshot` (a commit, in the store).
fn source_kind(target: &str, commit: Option<&str>) -> &'static str {
    if commit.is_some() {
        "snapshot"
    } else if target.replace('\\', "/").contains(&format!("/{}/", crate::config::WORK_DIR)) {
        "draft"
    } else {
        "working-tree"
    }
}

/// How a source repo skill's link deploys, for people: `main (working tree, live)`,
/// `terse (draft, live)`, `verbose @ 3f2a1c9 (snapshot)`; `copy` instead of `live` for a
/// copy (edits show only after re-linking); `pinned` when it does not follow the default.
pub fn describe(branch: Option<&str>, pinned: bool, source: &str, commit: Option<&str>, copy: bool) -> String {
    let b = branch.unwrap_or("detached");
    let pin = if pinned { ", pinned" } else { "" };
    let live = if copy { "copy" } else { "live" };
    match (source, commit) {
        ("snapshot", Some(c)) => format!("{b} @ {} (snapshot{pin})", &c[..c.len().min(7)]),
        ("draft", _) => format!("{b} (draft, {live}{pin})"),
        _ => format!("{b} (working tree, {live}{pin})"),
    }
}

#[derive(Debug, Serialize)]
pub struct UnlinkReport {
    pub removed: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UnlinkOptions<'a> {
    pub to: Option<&'a str>,
    pub global: bool,
    /// Every source repo's links (or, for `untry`, every trial), everywhere.
    pub all: bool,
}

/// Placements made by `link` (source repo skills).
const DEV: &str = "WHERE origin='source-repo'";
/// Placements made by `try`.
const TRIALS: &str = "WHERE origin IN ('trial','link')";

fn placement_name(p: &crate::state::Placement) -> String {
    Path::new(&p.path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Whether a placement is the skill `input` names: its key, its directory name, or an
/// `owner/repo//name` reference to it (matched without the network).
fn matches(p: &crate::state::Placement, input: &str) -> bool {
    if p.skill == input || placement_name(p) == input {
        return true;
    }
    if let Some((repo, sel)) = input.split_once("//") {
        let sel = sel.split('@').next().unwrap_or(sel);
        let repo = if repo.split('/').next().is_some_and(|h| h.contains('.')) { repo.to_string() } else { format!("github.com/{repo}") };
        let (pr, ppath) = p.skill.split_once("//").unwrap_or(("", ""));
        return pr.eq_ignore_ascii_case(&repo) && (ppath == sel || ppath.ends_with(&format!("/{sel}")));
    }
    false
}

fn narrowed(ctx: &Ctx, o: &UnlinkOptions) -> Result<Option<String>> {
    Ok(match (o.to, o.global) {
        (None, false) => None,
        _ => Some(scope_for(ctx, &LinkOptions { to: o.to, global: o.global, ..Default::default() }, false)?.key()),
    })
}

fn remove(ctx: &Ctx, filter: &str, keep: impl Fn(&crate::state::Placement) -> bool) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    for p in ctx.state.placements(filter, &[])? {
        if keep(&p) {
            deploy::remove_placement(ctx, &p)?;
            removed.push(p.path);
        }
    }
    // The store is a cache: drop what no link needs any more.
    let _ = store::gc(ctx, false);
    Ok(removed)
}

/// `tricks unlink [skill]`: remove links of source repo skills — one skill's, or with no
/// skill all of the current source repo's (`--all`: every source repo's).
pub fn unlink(ctx: &Ctx, input: Option<&str>, o: &UnlinkOptions) -> Result<UnlinkReport> {
    let scope = narrowed(ctx, o)?;
    let ws = crate::source_repo::current(ctx)?;
    if let Some(i) = input
        && ctx.state.placements(TRIALS, &[])?.iter().any(|p| matches(p, i))
        && !ctx.state.placements(DEV, &[])?.iter().any(|p| matches(p, i))
    {
        bail!("`{i}` is a trial; remove it with `tricks untry {i}`");
    }
    let repo_prefix = match (&ws, input, o.all) {
        (_, Some(_), _) | (_, None, true) => None,
        (Some(ws), None, false) => Some(format!("ws:{}//", ws.root.display())),
        (None, None, false) => bail!("not inside a source repo: name a skill, or pass --all to unlink every source repo's links"),
    };
    let key = input.and_then(|i| repo_target(ctx, i).ok().flatten()).map(|t| t.skill);
    let removed = remove(ctx, DEV, |p| {
        scope.as_ref().is_none_or(|s| &p.scope == s)
            && repo_prefix.as_ref().is_none_or(|pre| p.skill.starts_with(pre))
            && input.is_none_or(|i| key.as_deref() == Some(p.skill.as_str()) || (key.is_none() && matches(p, i)))
    })?;
    if let (true, Some(i)) = (removed.is_empty(), input) {
        bail!("no links of `{i}`");
    }
    Ok(UnlinkReport { removed })
}

/// `tricks untry [skill]`: remove trials — one skill's (wherever it is tried), or with no
/// skill those in the current project; `--global` / `--to` pick another place, `--all`
/// removes every trial.
pub fn untry(ctx: &Ctx, input: Option<&str>, o: &UnlinkOptions) -> Result<UnlinkReport> {
    if let Some(i) = input
        && repo_target(ctx, i)?.is_some()
    {
        bail!("`{i}` is a skill of this source repo; remove its links with `tricks unlink {i}`");
    }
    let scope = match (narrowed(ctx, o)?, input, o.all) {
        (Some(s), _, _) => Some(s),
        (None, None, false) => Some(crate::paths::canon(&ctx.opts.cwd)?.to_string_lossy().to_string()),
        _ => None,
    };
    let removed = remove(ctx, TRIALS, |p| scope.as_ref().is_none_or(|s| &p.scope == s) && input.is_none_or(|i| matches(p, i)))?;
    if let (true, Some(i)) = (removed.is_empty(), input) {
        bail!("`{i}` is not being tried");
    }
    Ok(UnlinkReport { removed })
}

#[derive(Debug, Serialize)]
pub struct LinkInfo {
    pub skill: String,
    /// The skill's directory name in the agent directory.
    pub name: String,
    pub agent: String,
    /// `global` (user-level agent directories) or a project path
    pub scope: String,
    pub path: String,
    pub mode: String,
    /// dev (a source repo skill, `link`) | trial (`try`)
    pub kind: String,
    /// ok | missing | replaced | target-missing | project-missing | drifted
    pub health: String,
    /// Source repo skills: the branch the link deploys, and whether it is pinned to it
    /// (`link <skill>@<branch>`) rather than following the skill's default.
    pub branch: Option<String>,
    pub pinned: bool,
    /// Source repo skills: working-tree | draft | snapshot
    pub source: Option<String>,
    /// The commit of a snapshot.
    pub commit: Option<String>,
}

fn info(p: crate::state::Placement) -> LinkInfo {
    let dev = p.origin == "source-repo";
    LinkInfo {
        source: dev.then(|| source_kind(&p.target, p.commit.as_deref()).to_string()),
        branch: p.branch.clone().filter(|_| dev),
        pinned: p.pin.is_some(),
        commit: p.commit.clone(),
        health: deploy::health(&p),
        kind: if p.origin == "source-repo" { "dev".into() } else { "trial".into() },
        name: placement_name(&p),
        skill: p.skill,
        agent: p.agent,
        scope: p.scope,
        path: p.path,
        mode: p.mode,
    }
}

/// Links of source repo skills: those of `repo`, or of every source repo.
pub fn dev_links(ctx: &Ctx, repo: Option<&crate::source_repo::SourceRepo>) -> Result<Vec<LinkInfo>> {
    let prefix = repo.map(|ws| format!("ws:{}//", ws.root.display()));
    Ok(ctx
        .state
        .placements(DEV, &[])?
        .into_iter()
        .filter(|p| prefix.as_ref().is_none_or(|pre| p.skill.starts_with(pre)))
        .map(info)
        .collect())
}

/// Trials: in the current project and at user level, or (`all`) everywhere.
pub fn trials(ctx: &Ctx, all: bool) -> Result<Vec<LinkInfo>> {
    let here = crate::paths::canon(&ctx.opts.cwd).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    Ok(ctx.state.placements(TRIALS, &[])?.into_iter().filter(|p| all || p.scope == "global" || p.scope == here).map(info).collect())
}
