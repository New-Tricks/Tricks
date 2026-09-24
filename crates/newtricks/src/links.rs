//! Test deployments to any target (spec §8): `link` / `unlink`.

use crate::agents;
use crate::ctx::Ctx;
use crate::deploy::{self, PlaceRequest, Scope};
use crate::resolve::{Fetch, resolve_skill};
use crate::skill::SkillDoc;
use crate::store;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct LinkReport {
    pub skill: String,
    pub name: String,
    pub scope: String,
    pub placements: Vec<(String, String, String)>,
}

/// What a link points at.
pub struct LinkTarget {
    pub skill: String,
    pub name: String,
    pub dir: PathBuf,
    pub tree: Option<String>,
    pub commit: Option<String>,
    pub dev: bool,
}

pub fn scope_from(ctx: &Ctx, to: Option<&str>, global: bool) -> Result<Scope> {
    match (to, global) {
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
        (None, false) => bail!("specify a target: --to <project-path> or --global"),
    }
}

/// Resolve `input` into something linkable: a local skill directory (dev mode), a
/// workspace skill (dev mode, active variant), or a remote skill (store).
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
        let skill = crate::workspace::skill_key_for_dir(&dir).unwrap_or_else(|| format!("local:{}", dir.display()));
        return Ok(LinkTarget { skill, name, dir, tree: None, commit: None, dev: true });
    }
    if let Some(t) = crate::workspace::link_target_for_name(ctx, input)? {
        return Ok(t);
    }
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    let r = resolve_skill(ctx, &spec, Fetch::IfStale)?;
    let dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    Ok(LinkTarget {
        skill: r.id.to_string(),
        name: crate::workbench::placement_name(&r.name, &r.id),
        dir,
        tree: Some(r.tree.clone()),
        commit: Some(r.reference.commit.clone()),
        dev: false,
    })
}

pub fn link(
    ctx: &Ctx,
    input: &str,
    to: Option<&str>,
    global: bool,
    agent_names: &[String],
    copy: bool,
    shadow: bool,
) -> Result<LinkReport> {
    let scope = scope_from(ctx, to, global)?;
    let t = link_target(ctx, input)?;
    let agents_sel = if agent_names.is_empty() {
        crate::workbench::default_agents(&crate::workbench::load_manifest(ctx)?)?
    } else {
        agents::parse_list(agent_names)?
    };
    if t.dev && agents_sel.iter().any(|a| !a.follows_links(&t.dir)) && !copy {
        ctx.ui.warn("some selected agents cannot follow links here; they get a copy, so live edits will not show until you re-link");
    }
    let mut placements = Vec::new();
    for a in agents_sel {
        let p = deploy::place(
            ctx,
            &PlaceRequest {
                skill: t.skill.clone(),
                origin: "link",
                agent: a,
                scope: scope.clone(),
                name: t.name.clone(),
                target: t.dir.clone(),
                tree: t.tree.clone(),
                commit: t.commit.clone(),
                force_copy: copy,
                shadow,
            },
        )?;
        placements.push((a.id.to_string(), p.path, p.mode));
    }
    Ok(LinkReport { skill: t.skill, name: t.name, scope: scope.key(), placements })
}

#[derive(Debug, Serialize)]
pub struct UnlinkReport {
    pub removed: Vec<String>,
}

pub fn unlink(ctx: &Ctx, input: Option<&str>, to: Option<&str>, global: bool, all: bool) -> Result<UnlinkReport> {
    let scope = if to.is_some() || global { Some(scope_from(ctx, to, global)?) } else { None };
    let skill_key = match input {
        Some(i) => Some(link_target(ctx, i).map(|t| t.skill).unwrap_or_else(|_| i.to_string())),
        None if all => None,
        None => bail!("name a skill to unlink, or pass --all"),
    };
    let mut removed = Vec::new();
    for p in ctx.state.placements("WHERE origin IN ('link','workspace')", &[])? {
        if let Some(s) = &scope
            && p.scope != s.key()
        {
            continue;
        }
        if let Some(k) = &skill_key {
            let name_match = Path::new(&p.path).file_name().map(|n| n.to_string_lossy() == k.as_str()).unwrap_or(false);
            if &p.skill != k && !name_match {
                continue;
            }
        }
        deploy::remove_placement(ctx, &p)?;
        removed.push(p.path);
    }
    if removed.is_empty() && input.is_some() {
        bail!("no matching test deployments");
    }
    Ok(UnlinkReport { removed })
}
