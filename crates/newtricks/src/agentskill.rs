//! The bundled `new-tricks` agent skill (spec §12): install, remove, status, and the
//! one-time offer shown by the CLI and the VS Code extension.

use crate::agents::{self, Agent};
use crate::ctx::Ctx;
use crate::deploy::{self, Scope};
use anyhow::Result;
use serde::Serialize;

pub const KEY: &str = "bundled:new-tricks";
const OFFERED: &str = "agent_skill_offered";

#[derive(Debug, Serialize)]
pub struct Status {
    pub installed: Vec<(String, String)>,
    pub offered: bool,
    /// True when the installed copy is older than the one bundled in this binary.
    pub outdated: bool,
}

pub fn status(ctx: &Ctx) -> Result<Status> {
    let placements = ctx.state.placements("WHERE skill=?1", &[&KEY])?;
    let current = bundled_tree()?;
    Ok(Status {
        outdated: placements.iter().any(|p| p.tree.as_deref() != Some(current.as_str())),
        installed: placements.into_iter().map(|p| (p.agent, p.path)).collect(),
        offered: ctx.state.meta_get(OFFERED)?.is_some(),
    })
}

fn bundled_tree() -> Result<String> {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("SKILL.md"), crate::source_repo::AGENT_SKILL)?;
    Ok(crate::treehash::tree_hash(tmp.path())?.unwrap_or_default())
}

pub fn install(ctx: &Ctx, agent_names: &[String]) -> Result<Vec<String>> {
    let sel: Vec<&'static Agent> = if agent_names.is_empty() {
        crate::user::default_agents(&crate::user::load_manifest(ctx)?)?
    } else {
        agents::parse_list(agent_names)?
    };
    mark_offered(ctx)?;
    crate::source_repo::install_agent_skill(ctx, &sel, &Scope::Global)
}

pub fn remove(ctx: &Ctx) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for p in ctx.state.placements("WHERE skill=?1", &[&KEY])? {
        deploy::remove_placement(ctx, &p)?;
        out.push(p.path);
    }
    mark_offered(ctx)?;
    Ok(out)
}

pub fn mark_offered(ctx: &Ctx) -> Result<()> {
    ctx.state.meta_set(OFFERED, &crate::state::now().to_string())
}

/// Offer only once, and never when it is already installed.
pub fn should_offer(ctx: &Ctx) -> Result<bool> {
    let s = status(ctx)?;
    Ok(!s.offered && s.installed.is_empty())
}

/// Refresh installed copies after an upgrade of the binary, each in its own scope.
pub fn refresh_if_outdated(ctx: &Ctx) -> Result<usize> {
    if !status(ctx)?.outdated {
        return Ok(0);
    }
    let mut n = 0;
    for p in ctx.state.placements("WHERE skill=?1", &[&KEY])? {
        if let Ok(a) = agents::get(&p.agent) {
            n += crate::source_repo::install_agent_skill(ctx, &[a], &Scope::from_key(&p.scope))?.len();
        }
    }
    Ok(n)
}
