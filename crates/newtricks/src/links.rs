//! Links (spec §8): deploy source repo skills — and upstream skills under trial — into a
//! project or the user-level agent directories to test them with real agents.

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

/// Resolve `input` into something linkable: a local skill directory (dev mode), a
/// source repo skill (dev mode, active variant, or `name@branch`), or an upstream skill
/// (a trial, from the store).
pub fn link_target(ctx: &Ctx, input: &str) -> Result<LinkTarget> {
    let p = Path::new(input);
    let local = if p.is_absolute() { p.to_path_buf() } else { ctx.opts.cwd.join(p) };
    if local.join("SKILL.md").is_file()
        && (input.starts_with('.') || input.starts_with('/') || input.contains(std::path::MAIN_SEPARATOR) || !input.contains("//"))
    {
        let dir = crate::paths::canon(&local)?;
        let doc = SkillDoc::parse(&std::fs::read_to_string(dir.join("SKILL.md"))?);
        let folder = dir.file_name().unwrap().to_string_lossy().to_string();
        let name = doc.name.filter(|n| crate::id::valid_skill_name(n)).unwrap_or(folder);
        let skill = crate::source_repo::skill_key_for_dir(&dir).unwrap_or_else(|| format!("local:{}", dir.display()));
        return Ok(LinkTarget { skill, name, dir, tree: None, commit: None, dev: true, trial: false });
    }
    if let Some(t) = crate::source_repo::link_target_for_name(ctx, input)? {
        return Ok(t);
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
            crate::hosted::StoreOutcome::Redirect(git_id) => link_target(ctx, &git_id),
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

/// `tricks link [skill]`: link one skill, or with no skill every skill of the source repo.
pub fn link(ctx: &Ctx, input: Option<&str>, o: &LinkOptions) -> Result<LinkReport> {
    let agents_sel = agents_for(ctx, o.agents)?;
    let mut rep = LinkReport::default();
    let Some(input) = input else {
        let ws = crate::source_repo::current(ctx)?
            .context("name a skill to link, or run `tricks link` inside a source repo to link all of its skills")?;
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
    let t = link_target(ctx, input)?;
    let scope = scope_for(ctx, o, t.trial)?;
    if t.dev && agents_sel.iter().any(|a| !a.follows_links(&t.dir)) && !o.copy {
        ctx.ui.warn("some selected agents cannot follow links here; they get a copy, so live edits will not show until you re-link");
    }
    let origin = if t.trial { "link" } else { "source-repo" };
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
    rep.links.push(Linked { skill: t.skill, name: t.name, scope: scope.key(), trial: t.trial, placements });
    Ok(rep)
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
    /// User-scope installs left by New Tricks 0.2 and earlier.
    pub legacy: bool,
}

const LEGACY: &str = "WHERE origin='user' AND skill<>'bundled:new-tricks'";

/// `tricks unlink [skill]`: remove links of a skill; with no skill, every link of the
/// current source repo's skills (or everything with `--all`).
pub fn unlink(ctx: &Ctx, input: Option<&str>, o: &UnlinkOptions) -> Result<UnlinkReport> {
    let scope = match (o.to, o.global) {
        (None, false) => None,
        _ => Some(scope_for(ctx, &LinkOptions { to: o.to, global: o.global, ..Default::default() }, false)?),
    };
    let keys: Option<Vec<String>> = match input {
        Some(i) => Some(vec![link_target(ctx, i).map(|t| t.skill).unwrap_or_else(|_| i.to_string())]),
        None if o.all || o.legacy => None,
        None => match crate::source_repo::current(ctx)? {
            Some(ws) => Some(ws.manifest.skills.keys().map(|n| ws.skill_key(n)).collect()),
            None => bail!("name a skill to unlink, run it inside a source repo, or pass --all"),
        },
    };
    let filter = if o.legacy { LEGACY } else { "WHERE origin IN ('link','source-repo')" };
    let mut removed = Vec::new();
    for p in ctx.state.placements(filter, &[])? {
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
    /// dev (source repo or local skill) | trial (upstream skill)
    pub kind: String,
    /// ok | missing | replaced | target-missing | project-missing | drifted
    pub health: String,
}

/// Active links, for `status`.
pub fn list(ctx: &Ctx) -> Result<Vec<LinkInfo>> {
    Ok(ctx
        .state
        .placements("WHERE origin IN ('link','source-repo')", &[])?
        .into_iter()
        .map(|p| LinkInfo {
            health: deploy::health(&p),
            kind: if p.origin == "link" { "trial".into() } else { "dev".into() },
            skill: p.skill,
            agent: p.agent,
            scope: p.scope,
            path: p.path,
            mode: p.mode,
        })
        .collect())
}

/// User-scope installs left by New Tricks 0.2 and earlier (still deployed, no longer managed).
pub fn legacy(ctx: &Ctx) -> Result<Vec<LinkInfo>> {
    Ok(ctx
        .state
        .placements(LEGACY, &[])?
        .into_iter()
        .map(|p| LinkInfo {
            health: deploy::health(&p),
            kind: "legacy".into(),
            skill: p.skill,
            agent: p.agent,
            scope: p.scope,
            path: p.path,
            mode: p.mode,
        })
        .collect())
}
