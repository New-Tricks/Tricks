//! Placement of skills into agent directories (spec §8): links with copy fallback,
//! git hygiene via the local exclude file, collision refusal and `--shadow`.

use crate::agents::Agent;
use crate::ctx::Ctx;
use crate::git;
use crate::state::{Placement, now};
use crate::store::{copy_dir, remove_dir_force};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Global,
    Project(PathBuf),
}

impl Scope {
    pub fn key(&self) -> String {
        match self {
            Scope::Global => "global".into(),
            Scope::Project(p) => p.to_string_lossy().to_string(),
        }
    }
    pub fn from_key(k: &str) -> Scope {
        if k == "global" { Scope::Global } else { Scope::Project(PathBuf::from(k)) }
    }
}

#[derive(Debug, Clone)]
pub struct PlaceRequest<'a> {
    pub skill: String,
    pub origin: &'a str,
    pub agent: &'static Agent,
    pub scope: Scope,
    pub name: String,
    /// Directory to place (store entry or dev directory).
    pub target: PathBuf,
    pub tree: Option<String>,
    pub commit: Option<String>,
    pub force_copy: bool,
    pub shadow: bool,
}

pub fn agent_dir(ctx: &Ctx, agent: &Agent, scope: &Scope) -> PathBuf {
    match scope {
        Scope::Global => agent.user_path(&ctx.paths.home),
        Scope::Project(root) => agent.project_path(root),
    }
}

fn is_our_placement(dest: &Path, p: &Placement) -> bool {
    match std::fs::symlink_metadata(dest) {
        Ok(md) if md.file_type().is_symlink() => std::fs::read_link(dest).map(|t| t == Path::new(&p.target)).unwrap_or(false),
        Ok(md) if md.is_dir() => p.mode == "copy",
        _ => false,
    }
}

/// Place a skill for one agent. Idempotent for the same skill at the same path.
pub fn place(ctx: &Ctx, req: &PlaceRequest) -> Result<Placement> {
    let dir = agent_dir(ctx, req.agent, &req.scope);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join(&req.name);
    let dest_s = dest.to_string_lossy().to_string();
    let existing = ctx.state.placement_at(&dest_s)?;
    let mut shadow_backup = None;

    let occupied = std::fs::symlink_metadata(&dest).is_ok();
    match &existing {
        Some(p) if p.skill != req.skill => {
            bail!("{} is already used by {} (placed by New Tricks); remove it first", dest.display(), p.skill)
        }
        Some(p) => {
            shadow_backup = p.shadow_backup.clone();
            if occupied && !is_our_placement(&dest, p) {
                bail!("{} was modified outside New Tricks; refusing to overwrite (move it away or use `unlink` first)", dest.display());
            }
        }
        None if occupied => {
            if !req.shadow {
                bail!(
                    "{} already exists (not managed by New Tricks). Use --shadow to back it up and replace it; `unlink` restores it.",
                    dest.display()
                );
            }
            let backup = ctx.paths.backups().join(format!("{}-{}-{}", now(), req.agent.id, req.name));
            std::fs::create_dir_all(backup.parent().unwrap())?;
            std::fs::rename(&dest, &backup).with_context(|| format!("backing up {}", dest.display()))?;
            ctx.ui.info(&format!("backed up existing {} to {}", dest.display(), backup.display()));
            shadow_backup = Some(backup.to_string_lossy().to_string());
        }
        None => {}
    }

    let use_link = !req.force_copy && req.agent.follows_links(&req.target);
    let mode = if use_link { "link" } else { "copy" };
    swap_into_place(&req.target, &dest, use_link)?;

    // Git hygiene: exclude placements inside a repository (never touch .gitignore).
    let (exclude_file, exclude_entry) = match add_exclude(&dest) {
        Ok(Some((f, e))) => (Some(f), Some(e)),
        Ok(None) => (None, None),
        Err(e) => {
            ctx.ui.warn(&format!("could not update git exclude for {}: {e:#}", dest.display()));
            (None, None)
        }
    };

    let p = Placement {
        id: 0,
        skill: req.skill.clone(),
        origin: req.origin.to_string(),
        agent: req.agent.id.to_string(),
        scope: req.scope.key(),
        path: dest_s,
        mode: mode.to_string(),
        target: req.target.to_string_lossy().to_string(),
        tree: req.tree.clone(),
        commit: req.commit.clone(),
        shadow_backup,
        exclude_file,
        exclude_entry,
        created_at: now(),
    };
    ctx.state.insert_placement(&p)?;
    ctx.state.record_deployment(&p.skill, &p.agent, &p.scope, p.tree.as_deref(), p.commit.as_deref(), mode, "place")?;
    Ok(p)
}

/// Atomically replace `dest` with a link to (or copy of) `target`.
fn swap_into_place(target: &Path, dest: &Path, link: bool) -> Result<()> {
    let parent = dest.parent().unwrap();
    let tmp = parent.join(format!(".{}.tricks-tmp-{}", dest.file_name().unwrap().to_string_lossy(), std::process::id()));
    let _ = remove_path(&tmp);
    if link {
        make_link(target, &tmp)?;
    } else {
        copy_dir(target, &tmp)?;
    }
    let old_md = std::fs::symlink_metadata(dest);
    match old_md {
        Ok(md) if md.file_type().is_symlink() || md.is_file() => {
            // rename(2) atomically replaces a symlink with a symlink; for dir-over-link
            // we remove first.
            if link {
                std::fs::rename(&tmp, dest)?;
            } else {
                std::fs::remove_file(dest)?;
                std::fs::rename(&tmp, dest)?;
            }
        }
        Ok(_) => {
            let old = parent.join(format!(".{}.tricks-old-{}", dest.file_name().unwrap().to_string_lossy(), std::process::id()));
            std::fs::rename(dest, &old)?;
            std::fs::rename(&tmp, dest)?;
            remove_path(&old)?;
        }
        Err(_) => std::fs::rename(&tmp, dest)?,
    }
    Ok(())
}

#[cfg(unix)]
fn make_link(target: &Path, at: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, at).with_context(|| format!("linking {} -> {}", at.display(), target.display()))
}

#[cfg(windows)]
fn make_link(target: &Path, at: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(target, at).with_context(|| format!("linking {} -> {}", at.display(), target.display()))
}

pub fn remove_path(p: &Path) -> Result<()> {
    match std::fs::symlink_metadata(p) {
        Ok(md) if md.file_type().is_symlink() || md.is_file() => std::fs::remove_file(p)?,
        Ok(_) => remove_dir_force(p)?,
        Err(_) => {}
    }
    Ok(())
}

pub fn remove_placement(ctx: &Ctx, p: &Placement) -> Result<()> {
    let dest = PathBuf::from(&p.path);
    if std::fs::symlink_metadata(&dest).is_ok() {
        if is_our_placement(&dest, p) {
            remove_path(&dest)?;
        } else {
            ctx.ui.warn(&format!("{} was changed outside New Tricks; leaving it in place", dest.display()));
        }
    }
    if let Some(b) = &p.shadow_backup {
        let b = PathBuf::from(b);
        if b.exists() && std::fs::symlink_metadata(&dest).is_err() {
            std::fs::rename(&b, &dest).with_context(|| format!("restoring {}", dest.display()))?;
            ctx.ui.info(&format!("restored original {}", dest.display()));
        }
    }
    if let (Some(f), Some(e)) = (&p.exclude_file, &p.exclude_entry) {
        let others = ctx.state.placements("WHERE exclude_file=?1 AND exclude_entry=?2 AND id<>?3", &[f, e, &p.id])?.len();
        if others == 0 {
            remove_exclude(Path::new(f), e)?;
        }
    }
    ctx.state.delete_placement(p.id)?;
    ctx.state.record_deployment(&p.skill, &p.agent, &p.scope, p.tree.as_deref(), p.commit.as_deref(), &p.mode, "remove")?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Health {
    pub path: String,
    pub state: String,
}

/// ok | missing | replaced | target-missing | project-missing
pub fn health(p: &Placement) -> String {
    if p.scope != "global" && !Path::new(&p.scope).exists() {
        return "project-missing".into();
    }
    let dest = Path::new(&p.path);
    match std::fs::symlink_metadata(dest) {
        Err(_) => "missing".into(),
        Ok(md) if md.file_type().is_symlink() => match std::fs::read_link(dest) {
            Ok(t) if t == Path::new(&p.target) => {
                if Path::new(&p.target).exists() {
                    "ok".into()
                } else {
                    "target-missing".into()
                }
            }
            _ => "replaced".into(),
        },
        Ok(md) if md.is_dir() && p.mode == "copy" => match (crate::treehash::tree_hash(dest).ok().flatten(), &p.tree) {
            (Some(h), Some(t)) if &h != t && p.origin != "workspace" => "drifted".into(),
            _ => "ok".into(),
        },
        Ok(_) => "replaced".into(),
    }
}

// ---------------------------------------------------------------- exclude handling

const BEGIN: &str = "# >>> new-tricks (managed; do not edit)";
const END: &str = "# <<< new-tricks";

/// Add `dest` to the containing repository's exclude file. Returns (file, entry).
pub fn add_exclude(dest: &Path) -> Result<Option<(String, String)>> {
    let parent = dest.parent().unwrap();
    let Some(root) = git::repo_root(parent) else { return Ok(None) };
    let Some(file) = git::exclude_file(parent) else { return Ok(None) };
    let root = root.canonicalize().unwrap_or(root);
    let parent_c = parent.canonicalize().unwrap_or(parent.to_path_buf());
    let rel = parent_c.strip_prefix(&root).map(|r| r.join(dest.file_name().unwrap())).unwrap_or_else(|_| dest.to_path_buf());
    let entry = format!("/{}", rel.to_string_lossy().replace('\\', "/"));
    let mut text = std::fs::read_to_string(&file).unwrap_or_default();
    let (before, mut block, after) = split_block(&text);
    if !block.iter().any(|l| l == &entry) {
        block.push(entry.clone());
        text = join_block(&before, &block, &after);
        if let Some(p) = file.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&file, text)?;
    }
    Ok(Some((file.to_string_lossy().to_string(), entry)))
}

pub fn remove_exclude(file: &Path, entry: &str) -> Result<()> {
    let Ok(text) = std::fs::read_to_string(file) else { return Ok(()) };
    let (before, mut block, after) = split_block(&text);
    block.retain(|l| l != entry);
    std::fs::write(file, join_block(&before, &block, &after))?;
    Ok(())
}

fn split_block(text: &str) -> (String, Vec<String>, String) {
    match (text.find(BEGIN), text.find(END)) {
        (Some(b), Some(e)) if e > b => {
            let inner = &text[b + BEGIN.len()..e];
            let block = inner.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            let after_start = e + END.len();
            let after = text[after_start..].trim_start_matches('\n').to_string();
            (text[..b].to_string(), block, after)
        }
        _ => (text.to_string(), Vec::new(), String::new()),
    }
}

fn join_block(before: &str, block: &[String], after: &str) -> String {
    let mut out = before.to_string();
    if block.is_empty() {
        let mut s = out.trim_end_matches('\n').to_string();
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(after);
        return s;
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(BEGIN);
    out.push('\n');
    for l in block {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(END);
    out.push('\n');
    out.push_str(after);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_block_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("exclude");
        std::fs::write(&f, "# user stuff\n*.log\n").unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        let (b, mut blk, a) = split_block(&text);
        blk.push("/.claude/skills/pdf".into());
        std::fs::write(&f, join_block(&b, &blk, &a)).unwrap();
        let s = std::fs::read_to_string(&f).unwrap();
        assert!(s.starts_with("# user stuff\n*.log\n# >>> new-tricks"));
        remove_exclude(&f, "/.claude/skills/pdf").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "# user stuff\n*.log\n");
    }
}
