//! Content-addressed, read-only store of exact skill revisions (spec §8).

use crate::ctx::Ctx;
use crate::git::Mirror;
use crate::treehash::tree_hash;
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub fn entry_path(ctx: &Ctx, tree: &str) -> PathBuf {
    ctx.paths.store().join(tree)
}

/// Materialize `path` at `commit` from a mirror into the store keyed by its tree SHA.
pub fn from_mirror(ctx: &Ctx, m: &Mirror, commit: &str, path: &str, tree: &str) -> Result<PathBuf> {
    let dest = entry_path(ctx, tree);
    if dest.exists() {
        return Ok(dest);
    }
    let tmp = ctx.paths.store().join(format!(".tmp-{tree}-{}", std::process::id()));
    let _ = remove_dir_force(&tmp);
    m.export_dir(commit, path, &tmp)?;
    match tree_hash(&tmp)? {
        Some(h) if h == tree => {}
        Some(h) => ctx.ui.warn(&format!(
            "exported content of {}//{path} hashes to {h}, expected {tree} (export attributes?); storing under the upstream tree id",
            m.source
        )),
        None => bail!("{}//{path} at {commit} is empty", m.source),
    }
    finalize(&tmp, &dest)?;
    Ok(dest)
}

/// Copy a local directory into the store; returns (store path, tree hash).
pub fn from_dir(ctx: &Ctx, src: &Path) -> Result<(PathBuf, String)> {
    let tree = tree_hash(src)?.with_context(|| format!("{} is empty", src.display()))?;
    let dest = entry_path(ctx, &tree);
    if dest.exists() {
        return Ok((dest, tree));
    }
    let tmp = ctx.paths.store().join(format!(".tmp-{tree}-{}", std::process::id()));
    let _ = remove_dir_force(&tmp);
    copy_dir(src, &tmp)?;
    finalize(&tmp, &dest)?;
    Ok((dest, tree))
}

/// Store a single-file skill (well-known `skill-md`) under its tree hash.
pub fn from_skill_md(ctx: &Ctx, content: &[u8]) -> Result<(PathBuf, String)> {
    let tmp = tempfile::tempdir_in(ctx.paths.store())?;
    std::fs::write(tmp.path().join("SKILL.md"), content)?;
    from_dir(ctx, tmp.path())
}

fn finalize(tmp: &Path, dest: &Path) -> Result<()> {
    set_readonly(tmp, true)?;
    match std::fs::rename(tmp, dest) {
        Ok(()) => Ok(()),
        Err(_) if dest.exists() => {
            let _ = remove_dir_force(tmp); // raced with another process: same content
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("moving into store {}", dest.display())),
    }
}

pub fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for e in walkdir::WalkDir::new(src).min_depth(1).into_iter().filter_entry(|e| e.file_name() != ".git") {
        let e = e?;
        let rel = e.path().strip_prefix(src).unwrap();
        let to = dest.join(rel);
        let ft = e.file_type();
        if ft.is_dir() {
            std::fs::create_dir_all(&to)?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(e.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &to)?;
            #[cfg(not(unix))]
            {
                let resolved = e.path().parent().unwrap().join(&target);
                if resolved.is_file() {
                    std::fs::copy(&resolved, &to)?;
                }
            }
        } else {
            std::fs::copy(e.path(), &to).with_context(|| format!("copying {}", e.path().display()))?;
            // Copies of read-only store files must stay writable in the destination.
            let mut perm = std::fs::metadata(&to)?.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            std::fs::set_permissions(&to, perm)?;
        }
    }
    Ok(())
}

pub fn set_readonly(dir: &Path, ro: bool) -> Result<()> {
    // Files first, then directories bottom-up (a read-only dir blocks changes inside it).
    let mut dirs = Vec::new();
    for e in walkdir::WalkDir::new(dir).contents_first(true) {
        let e = e?;
        if e.file_type().is_symlink() {
            continue;
        }
        if e.file_type().is_dir() {
            dirs.push(e.path().to_path_buf());
            continue;
        }
        set_mode(e.path(), ro, false)?;
    }
    if ro {
        for d in &dirs {
            set_mode(d, true, true)?;
        }
    } else {
        for d in dirs.iter().rev() {
            set_mode(d, false, true)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(p: &Path, ro: bool, is_dir: bool) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let m = std::fs::metadata(p)?.permissions().mode();
    let new = if ro {
        m & !0o222
    } else if is_dir {
        m | 0o700
    } else {
        m | 0o200
    };
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(new))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_mode(p: &Path, ro: bool, is_dir: bool) -> Result<()> {
    if is_dir {
        return Ok(());
    }
    let mut perm = std::fs::metadata(p)?.permissions();
    perm.set_readonly(ro);
    std::fs::set_permissions(p, perm)?;
    Ok(())
}

pub fn remove_dir_force(p: &Path) -> Result<()> {
    if !p.exists() && std::fs::symlink_metadata(p).is_err() {
        return Ok(());
    }
    let _ = set_readonly(p, false);
    std::fs::remove_dir_all(p)?;
    Ok(())
}

/// Trees that must be kept: anything a placement points at, and the base and last
/// fetched upstream snapshots of vendored skills whose upstream is catalog-hosted (not in
/// git, so they cannot be rebuilt from a mirror).
pub fn referenced(ctx: &Ctx) -> Result<BTreeSet<String>> {
    let mut keep = BTreeSet::new();
    let c = &ctx.state.conn;
    let mut st = c.prepare("SELECT tree FROM placements WHERE tree IS NOT NULL")?;
    for r in st.query_map([], |r| r.get::<_, String>(0))? {
        keep.insert(r?);
    }
    keep.extend(crate::source_repo::hosted_snapshots(ctx)?);
    Ok(keep)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct GcReport {
    pub removed: Vec<String>,
    pub kept: usize,
}

pub fn gc(ctx: &Ctx, dry_run: bool) -> Result<GcReport> {
    let keep = referenced(ctx)?;
    let mut rep = GcReport::default();
    for e in std::fs::read_dir(ctx.paths.store())? {
        let e = e?;
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(".tmp-") {
            if !dry_run {
                let _ = remove_dir_force(&e.path());
            }
            continue;
        }
        if keep.contains(&name) {
            rep.kept += 1;
            continue;
        }
        if !dry_run {
            remove_dir_force(&e.path())?;
        }
        rep.removed.push(name);
    }
    Ok(rep)
}
