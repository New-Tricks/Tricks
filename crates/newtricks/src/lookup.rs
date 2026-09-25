//! Turning user input into skill specs: canonical/short IDs, URLs (with slash-branch
//! resolution) and bare names (user installs, then the search index).

use crate::ctx::Ctx;
use crate::id::{Normalized, SkillSpec, normalize_url};
use crate::resolve::{Fetch, mirror_refs, open_mirror};
use anyhow::{Result, bail};
use rusqlite::params;

pub fn spec_from_input(ctx: &Ctx, input: &str) -> Result<SkillSpec> {
    if let Some(n) = normalize_url(input)? {
        return match n {
            Normalized::Skill(s) => Ok(s),
            Normalized::Source(src, _) => bail!("`{input}` is a repository ({src}); add `//<skill>` or a path to a skill"),
            Normalized::Ambiguous(a) => {
                // Prefer live refs (the mirror may predate a new branch).
                let names: Vec<String> = match (!ctx.opts.offline).then(|| crate::git::ls_remote(&crate::git::clone_url(&a.source))) {
                    Some(Ok(r)) => r.all_names(),
                    _ => {
                        let m = open_mirror(ctx, &a.source, Fetch::IfStale)?;
                        let refs = mirror_refs(&m)?;
                        refs.heads.keys().chain(refs.tags.keys()).cloned().collect()
                    }
                };
                Ok(a.resolve(&names))
            }
        };
    }
    if input.contains("//") {
        return SkillSpec::parse(input);
    }
    // Bare name: installed skills first, then the index.
    let (name, reference) = match input.rsplit_once('@') {
        Some((n, r)) => (n, Some(r.to_string())),
        None => (input, None),
    };
    let lock = crate::config::UserLock::load(&ctx.paths.user_lock())?;
    let installed: Vec<&crate::config::LockedSkill> =
        lock.skills.iter().filter(|s| s.name == name || s.id.rsplit('/').next() == Some(name)).collect();
    if installed.len() == 1 {
        let mut s = SkillSpec::parse(&installed[0].id)?;
        s.reference = reference;
        return Ok(s);
    }
    let mut st = ctx.state.conn.prepare("SELECT id, description FROM skills WHERE name=?1 OR folder=?1 ORDER BY (name=?1) DESC")?;
    let rows: Vec<(String, String)> =
        st.query_map(params![name], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?.collect::<Result<_, _>>()?;
    let pick = match rows.len() {
        0 => bail!("no skill named `{name}` is installed or indexed; try `tricks search {name}` or use owner/repo//{name}"),
        1 => 0,
        _ => {
            let opts: Vec<String> = rows.iter().map(|(id, d)| format!("{id} — {}", d.chars().take(70).collect::<String>())).collect();
            match ctx.ui.select(&format!("`{name}` matches several skills:"), &opts)? {
                Some(i) => i,
                None => bail!("`{name}` is ambiguous: {}", rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", ")),
            }
        }
    };
    let mut s = SkillSpec::parse(&rows[pick].0)?;
    s.reference = reference;
    Ok(s)
}
