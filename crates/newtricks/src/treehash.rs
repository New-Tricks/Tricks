//! Git-compatible tree hashing of a directory, so local content can be compared with
//! upstream tree SHAs from git or the GitHub trees API (spec §5 content identity).

use anyhow::{Context, Result};
use sha1::{Digest, Sha1};
use std::path::Path;

/// Compute the git tree SHA-1 of `dir`, ignoring `.git` and empty directories.
/// Returns `None` if the directory contains no files (git cannot represent it).
pub fn tree_hash(dir: &Path) -> Result<Option<String>> {
    Ok(hash_dir(dir)?.map(hex::encode))
}

fn hash_dir(dir: &Path) -> Result<Option<[u8; 20]>> {
    let mut entries: Vec<(Vec<u8>, &'static str, [u8; 20])> = Vec::new();
    let rd = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for e in rd {
        let e = e?;
        let name = e.file_name();
        let name_s = name.to_string_lossy().to_string();
        if name_s == ".git" {
            continue;
        }
        let ft = e.file_type()?;
        let path = e.path();
        if ft.is_symlink() {
            let target = std::fs::read_link(&path)?;
            let content = target.to_string_lossy().replace('\\', "/");
            entries.push((name_s.into_bytes(), "120000", blob_hash(content.as_bytes())));
        } else if ft.is_dir() {
            if let Some(h) = hash_dir(&path)? {
                entries.push((name_s.into_bytes(), "40000", h));
            }
        } else if ft.is_file() {
            let content = std::fs::read(&path)?;
            let mode = if is_executable(&e.metadata()?) { "100755" } else { "100644" };
            entries.push((name_s.into_bytes(), mode, blob_hash(&content)));
        }
    }
    if entries.is_empty() {
        return Ok(None);
    }
    // Git orders tree entries by name, comparing directories as if suffixed with '/'.
    entries.sort_by_key(|a| sort_key(&a.0, a.1));
    let mut body = Vec::new();
    for (name, mode, h) in &entries {
        body.extend_from_slice(mode.as_bytes());
        body.push(b' ');
        body.extend_from_slice(name);
        body.push(0);
        body.extend_from_slice(h);
    }
    Ok(Some(object_hash("tree", &body)))
}

fn sort_key(name: &[u8], mode: &str) -> Vec<u8> {
    let mut k = name.to_vec();
    if mode == "40000" {
        k.push(b'/');
    }
    k
}

pub fn blob_hash(content: &[u8]) -> [u8; 20] {
    object_hash("blob", content)
}

pub fn blob_hash_hex(content: &[u8]) -> String {
    hex::encode(blob_hash(content))
}

fn object_hash(kind: &str, body: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(format!("{kind} {}\0", body.len()).as_bytes());
    h.update(body);
    h.finalize().into()
}

#[cfg(unix)]
fn is_executable(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    m.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_m: &std::fs::Metadata) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn matches_git() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        std::fs::create_dir_all(d.join("scripts")).unwrap();
        std::fs::create_dir_all(d.join("a-b")).unwrap();
        std::fs::write(d.join("SKILL.md"), "---\nname: x\n---\nhello\n").unwrap();
        std::fs::write(d.join("scripts/run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(d.join("a-b/z.txt"), "z").unwrap();
        std::fs::write(d.join("a.txt"), "a").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d.join("scripts/run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let ours = tree_hash(d).unwrap().unwrap();
        let git = |args: &[&str]| {
            let o = Command::new("git").args(args).current_dir(d).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8(o.stdout).unwrap().trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        let theirs = git(&["write-tree"]);
        assert_eq!(ours, theirs);
    }
}
