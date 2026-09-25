//! The user config (`~/.config/newtricks/tricks.toml`): settings, catalogs and
//! registered source repos. New Tricks does not manage skills at user scope; it links
//! source repo skills (and upstream skills under trial) for testing.

use crate::agents::{self, Agent};
use crate::config::UserConfig;
use crate::ctx::Ctx;
use crate::id::{SkillId, valid_skill_name};
use anyhow::Result;

pub fn config(ctx: &Ctx) -> Result<UserConfig> {
    UserConfig::load(&ctx.paths.user_config())
}

pub fn default_agents(c: &UserConfig) -> Result<Vec<&'static Agent>> {
    let a = agents::parse_list(&c.settings.agents)?;
    Ok(if a.is_empty() { vec![agents::get("claude")?] } else { a })
}

/// Placement directory name: frontmatter name when valid, else folder name.
pub fn placement_name(name: &str, id: &SkillId) -> String {
    if valid_skill_name(name) { name.to_string() } else { id.folder_name().to_string() }
}
