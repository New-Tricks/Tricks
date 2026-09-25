//! `tricks statusline`: a one-line summary for agent status lines (spec §9).
//! Reads cached state only; never touches the network, so it cannot slow a prompt.

use crate::ctx::Ctx;
use anyhow::Result;

pub fn line(ctx: &Ctx) -> Result<String> {
    let c = &ctx.state.conn;
    let mut parts = Vec::new();
    if let Some(ws) = crate::source_repo::current(ctx)? {
        let root = ws.root.to_string_lossy().to_string();
        let merges: i64 = c.query_row("SELECT COUNT(*) FROM merges WHERE workspace=?1", [&root], |r| r.get(0))?;
        if merges > 0 {
            parts.push(format!("{merges} merge{} in progress", if merges == 1 { "" } else { "s" }));
        }
        let editing: i64 = c.query_row("SELECT COUNT(*) FROM meta WHERE key LIKE ?1", [format!("editing:{root}:%")], |r| r.get(0))?;
        if editing > 0 {
            parts.push(format!("editing {editing}"));
        }
    }
    let links: i64 = c.query_row("SELECT COUNT(*) FROM placements WHERE origin IN ('source-repo','trial','link')", [], |r| r.get(0))?;
    if links > 0 {
        parts.push(format!("{links} link{}", if links == 1 { "" } else { "s" }));
    }
    Ok(if parts.is_empty() { String::new() } else { format!("tricks: {}", parts.join(" · ")) })
}
