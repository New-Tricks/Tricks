//! `tricks statusline`: a one-line summary for agent status lines (spec §9).
//! Reads cached state only; never touches the network, so it cannot slow a prompt.

use crate::ctx::Ctx;
use anyhow::Result;

pub fn line(ctx: &Ctx) -> Result<String> {
    let c = &ctx.state.conn;
    let updates: i64 = c.query_row("SELECT COUNT(*) FROM pending_updates", [], |r| r.get(0))?;
    let merges: i64 = c.query_row("SELECT COUNT(*) FROM merges", [], |r| r.get(0))?;
    let links: i64 = c.query_row("SELECT COUNT(*) FROM placements WHERE origin='link'", [], |r| r.get(0))?;
    let mut parts = Vec::new();
    if updates > 0 {
        parts.push(format!("{updates} update{}", if updates == 1 { "" } else { "s" }));
    }
    if merges > 0 {
        parts.push(format!("{merges} merge{} in progress", if merges == 1 { "" } else { "s" }));
    }
    if links > 0 {
        parts.push(format!("{links} test link{}", if links == 1 { "" } else { "s" }));
    }
    Ok(if parts.is_empty() { String::new() } else { format!("tricks: {}", parts.join(" · ")) })
}
