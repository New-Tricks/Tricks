//! The bundled `new-tricks` agent skill (spec §12). `tricks init --agent-skill` installs
//! it into a source repo; to have it everywhere, install it with the ecosystem's tools
//! (`npx skills add new-tricks/tricks`), like any other published skill.

use crate::agents::Agent;
use crate::ctx::Ctx;
use crate::deploy::{self, PlaceRequest, Scope};
use crate::store;
use anyhow::Result;

pub const KEY: &str = "bundled:new-tricks";
pub const SKILL_MD: &str = include_str!("../../../skills/new-tricks/SKILL.md");

/// Place the bundled skill for `agents_sel` in `scope`.
pub fn install(ctx: &Ctx, agents_sel: &[&'static Agent], scope: &Scope) -> Result<Vec<String>> {
    let tmp = tempfile::tempdir()?;
    std::fs::write(tmp.path().join("SKILL.md"), SKILL_MD)?;
    let (dir, tree) = store::from_dir(ctx, tmp.path())?;
    let mut out = Vec::new();
    for a in agents_sel {
        let p = deploy::place(
            ctx,
            &PlaceRequest {
                skill: KEY.into(),
                origin: "agent-skill",
                agent: a,
                scope: scope.clone(),
                name: "new-tricks".into(),
                target: dir.clone(),
                tree: Some(tree.clone()),
                commit: None,
                force_copy: false,
                shadow: false,
                pin: None,
                branch: None,
            },
        )?;
        out.push(p.path);
    }
    Ok(out)
}
