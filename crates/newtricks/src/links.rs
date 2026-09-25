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
}

#[derive(Debug, Serialize)]
pub struct Linked {
    pub skill: String,
    pub name: String,
    pub scope: String,
    pub trial: bool,
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
        return Ok(LinkTarget { skill: format!("local:{}", dir.display()), name, dir, tree: None, commit: None, dev: true, trial: true });
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
            },
        )?;
        placements.push((a.id.to_string(), p.path, p.mode));
    }
    Ok(Linked { skill: t.skill, name: t.name, scope: scope.key(), trial: t.trial, placements })
}

#[derive(Debug, Serialize)]
pub struct UnlinkReport {
    pub removed: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UnlinkOptions<'a> {
    pub to: Option<&'a str>,
    pub global: bool,
    /// Every link, everywhere.
    pub all: bool,
}

/// Placements made by `link` and `try`.
const LINKS: &str = "WHERE origin IN ('source-repo','trial','link')";

/// `tricks unlink [skill]`: remove the links (and trials) of a skill; with no skill, every
/// link of the current source repo's skills (or everything with `--all`).
pub fn unlink(ctx: &Ctx, input: Option<&str>, o: &UnlinkOptions) -> Result<UnlinkReport> {
    let scope = match (o.to, o.global) {
        (None, false) => None,
        _ => Some(scope_for(ctx, &LinkOptions { to: o.to, global: o.global, ..Default::default() }, false)?),
    };
    let keys: Option<Vec<String>> = match input {
        Some(i) => Some(vec![target_key(ctx, i).unwrap_or_else(|| i.to_string())]),
        None if o.all => None,
        None => match crate::source_repo::current(ctx)? {
            Some(ws) => Some(ws.manifest.skills.keys().map(|n| ws.skill_key(n)).collect()),
            None => bail!("name a skill to unlink, run it inside a source repo, or pass --all"),
        },
    };
    let mut removed = Vec::new();
    for p in ctx.state.placements(LINKS, &[])? {
        if let Some(s) = &scope
            && p.scope != s.key()
        {
            continue;
        }
        if let Some(ks) = &keys {
            let name_match = |k: &String| Path::new(&p.path).file_name().map(|n| n.to_string_lossy() == k.as_str()).unwrap_or(false);
            if !ks.iter().any(|k| &p.skill == k || name_match(k)) {
                continue;
            }
        }
        deploy::remove_placement(ctx, &p)?;
        removed.push(p.path);
    }
    if removed.is_empty() && input.is_some() {
        bail!("no matching links");
    }
    // The store is a cache: drop what no link needs any more.
    let _ = store::gc(ctx, false);
    Ok(UnlinkReport { removed })
}

#[derive(Debug, Serialize)]
pub struct LinkInfo {
    pub skill: String,
    pub agent: String,
    pub scope: String,
    pub path: String,
    pub mode: String,
    /// dev (a source repo skill) | trial (`tricks try`)
    pub kind: String,
    /// ok | missing | replaced | target-missing | project-missing | drifted
    pub health: String,
}

/// Active links, for `status`.
pub fn list(ctx: &Ctx) -> Result<Vec<LinkInfo>> {
    Ok(ctx
        .state
        .placements(LINKS, &[])?
        .into_iter()
        .map(|p| LinkInfo {
            health: deploy::health(&p),
            kind: if p.origin == "source-repo" { "dev".into() } else { "trial".into() },
            skill: p.skill,
            agent: p.agent,
            scope: p.scope,
            path: p.path,
            mode: p.mode,
        })
        .collect())
}
