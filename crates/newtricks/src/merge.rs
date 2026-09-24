//! Three-way merge of an upstream skill directory into a customized copy (spec §10):
//! B = recorded base, C = workspace files (ours), U = latest upstream (theirs).

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    /// text | binary | deleted-upstream | deleted-locally | added-both
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct MergeOutcome {
    /// Files updated from upstream without conflict.
    pub updated: Vec<String>,
    pub added: Vec<String>,
    pub deleted: Vec<String>,
    /// Files where both sides changed and git merged them cleanly.
    pub merged: Vec<String>,
    pub conflicts: Vec<Conflict>,
}

impl MergeOutcome {
    pub fn is_clean(&self) -> bool {
        self.conflicts.is_empty()
    }
    pub fn changed(&self) -> bool {
        !(self.updated.is_empty()
            && self.added.is_empty()
            && self.deleted.is_empty()
            && self.merged.is_empty()
            && self.conflicts.is_empty())
    }
}

pub const UPSTREAM_SUFFIX: &str = ".upstream";

fn files(dir: &Path) -> BTreeSet<String> {
    let mut s = BTreeSet::new();
    if !dir.exists() {
        return s;
    }
    for e in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| e.file_name() != ".git").flatten() {
        if e.file_type().is_file() || e.file_type().is_symlink() {
            s.insert(e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/"));
        }
    }
    s
}

fn read(dir: &Path, rel: &str) -> Option<Vec<u8>> {
    std::fs::read(dir.join(rel)).ok()
}

fn exec_bit(dir: &Path, rel: &str) -> Option<bool> {
    let p = dir.join(rel);
    p.exists().then(|| crate::risk::is_exec(&p))
}

fn write_file(dir: &Path, rel: &str, content: &[u8], exec: Option<bool>) -> Result<()> {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&p, content)?;
    #[cfg(unix)]
    if let Some(x) = exec {
        use std::os::unix::fs::PermissionsExt;
        let mode = if x { 0o755 } else { 0o644 };
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = exec;
    Ok(())
}

/// Merge `base` → `theirs` changes into `ours` in place.
pub fn three_way(base: &Path, ours: &Path, theirs: &Path, labels: (&str, &str, &str)) -> Result<MergeOutcome> {
    let mut out = MergeOutcome::default();
    let all: BTreeSet<String> = files(base).into_iter().chain(files(ours)).chain(files(theirs)).collect();
    for rel in all {
        if rel.ends_with(UPSTREAM_SUFFIX) {
            continue;
        }
        let b = read(base, &rel);
        let o = read(ours, &rel);
        let t = read(theirs, &rel);
        if o == t {
            continue;
        }
        if b == t {
            continue; // upstream did not touch it: keep ours
        }
        if b == o {
            // Only upstream changed: take theirs (including additions and deletions).
            match &t {
                Some(content) => {
                    let exec = exec_bit(theirs, &rel);
                    write_file(ours, &rel, content, exec)?;
                    if o.is_some() { out.updated.push(rel.clone()) } else { out.added.push(rel.clone()) }
                }
                None => {
                    std::fs::remove_file(ours.join(&rel))?;
                    remove_empty_parents(ours, &rel);
                    out.deleted.push(rel.clone());
                }
            }
            continue;
        }
        // Both sides changed.
        match (&o, &t) {
            (Some(oc), Some(tc)) => {
                let binary = oc.contains(&0) || tc.contains(&0) || b.as_ref().map(|x| x.contains(&0)).unwrap_or(false);
                if binary {
                    write_file(ours, &format!("{rel}{UPSTREAM_SUFFIX}"), tc, None)?;
                    out.conflicts.push(Conflict { path: rel.clone(), kind: "binary".into() });
                    continue;
                }
                let (merged, n) = merge_text(b.as_deref().unwrap_or(b""), oc, tc, labels)?;
                write_file(ours, &rel, &merged, None)?;
                if n > 0 {
                    out.conflicts.push(Conflict { path: rel.clone(), kind: if b.is_none() { "added-both".into() } else { "text".into() } });
                } else {
                    out.merged.push(rel.clone());
                }
            }
            (Some(_), None) => out.conflicts.push(Conflict { path: rel.clone(), kind: "deleted-upstream".into() }),
            (None, Some(tc)) => {
                write_file(ours, &format!("{rel}{UPSTREAM_SUFFIX}"), tc, None)?;
                out.conflicts.push(Conflict { path: rel.clone(), kind: "deleted-locally".into() });
            }
            (None, None) => {}
        }
    }
    Ok(out)
}

fn remove_empty_parents(root: &Path, rel: &str) {
    let mut p = root.join(rel);
    while let Some(parent) = p.parent() {
        if parent == root {
            break;
        }
        if std::fs::remove_dir(parent).is_err() {
            break;
        }
        p = parent.to_path_buf();
    }
}

/// `git merge-file` on three blobs. Returns (result, number of conflicts).
pub fn merge_text(base: &[u8], ours: &[u8], theirs: &[u8], labels: (&str, &str, &str)) -> Result<(Vec<u8>, i32)> {
    let dir = tempfile::tempdir()?;
    let (pb, po, pt) = (dir.path().join("base"), dir.path().join("ours"), dir.path().join("theirs"));
    std::fs::write(&pb, base)?;
    std::fs::write(&po, ours)?;
    std::fs::write(&pt, theirs)?;
    let o = Command::new("git")
        .args(["merge-file", "-p", "-L", labels.0, "-L", labels.1, "-L", labels.2])
        .arg(&po)
        .arg(&pb)
        .arg(&pt)
        .output()
        .context("running git merge-file")?;
    let code = o.status.code().unwrap_or(-1);
    if code < 0 {
        bail!("git merge-file failed: {}", String::from_utf8_lossy(&o.stderr));
    }
    Ok((o.stdout, code))
}

/// Paths in `dir` that still contain conflict markers or `.upstream` sidecars.
pub fn unresolved(dir: &Path, conflicts: &[Conflict]) -> Vec<String> {
    let mut v = Vec::new();
    for c in conflicts {
        match c.kind.as_str() {
            "text" | "added-both" => {
                if let Ok(t) = std::fs::read_to_string(dir.join(&c.path))
                    && t.lines().any(|l| l.starts_with("<<<<<<< ") || l.starts_with(">>>>>>> "))
                {
                    v.push(format!("{} (conflict markers)", c.path));
                }
            }
            "binary" | "deleted-locally" => {
                if dir.join(format!("{}{UPSTREAM_SUFFIX}", c.path)).exists() {
                    v.push(format!("{} (remove {}{UPSTREAM_SUFFIX} once resolved)", c.path, c.path));
                }
            }
            _ => {}
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(d: &Path, rel: &str, s: &str) {
        let p = d.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn merges_non_overlapping_and_flags_overlaps() {
        let t = tempfile::tempdir().unwrap();
        let (b, o, th) = (t.path().join("b"), t.path().join("o"), t.path().join("t"));
        let base = "line1\nline2\nline3\nline4\nline5\n";
        w(&b, "SKILL.md", base);
        w(&o, "SKILL.md", "line1 mine\nline2\nline3\nline4\nline5\n");
        w(&th, "SKILL.md", "line1\nline2\nline3\nline4\nline5 theirs\n");
        w(&b, "ref.md", "a\n");
        w(&o, "ref.md", "a\n");
        w(&th, "ref.md", "a updated\n");
        w(&th, "new.md", "new\n");
        w(&b, "gone.md", "x\n");
        w(&o, "gone.md", "x\n");
        w(&b, "clash.md", "x\n");
        w(&o, "clash.md", "mine\n");
        w(&th, "clash.md", "theirs\n");
        w(&b, "deleted-up.md", "x\n");
        w(&o, "deleted-up.md", "edited\n");
        let r = three_way(&b, &o, &th, ("mine", "base", "upstream")).unwrap();
        assert_eq!(std::fs::read_to_string(o.join("SKILL.md")).unwrap(), "line1 mine\nline2\nline3\nline4\nline5 theirs\n");
        assert_eq!(r.merged, vec!["SKILL.md"]);
        assert_eq!(r.updated, vec!["ref.md"]);
        assert_eq!(r.added, vec!["new.md"]);
        assert_eq!(r.deleted, vec!["gone.md"]);
        assert!(!o.join("gone.md").exists());
        let kinds: Vec<(String, String)> = r.conflicts.iter().map(|c| (c.path.clone(), c.kind.clone())).collect();
        assert!(kinds.contains(&("clash.md".into(), "text".into())));
        assert!(kinds.contains(&("deleted-up.md".into(), "deleted-upstream".into())));
        let un = unresolved(&o, &r.conflicts);
        assert_eq!(un.len(), 1);
        w(&o, "clash.md", "resolved\n");
        assert!(unresolved(&o, &r.conflicts).is_empty());
    }
}
