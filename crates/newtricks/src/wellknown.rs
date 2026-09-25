//! `/.well-known/agent-skills/index.json` discovery (Agent Skills Discovery RFC v0.2.0):
//! `skill-md` and `archive` entries, mandatory digest verification, RFC 3986 URL
//! resolution, and safe archive extraction.

use crate::ctx::Ctx;
use crate::id::SourceId;
use crate::index::{self, IndexedSkill};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::Digest;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const SCHEMA_V02: &str = "https://schemas.agentskills.io/discovery/0.2.0/schema.json";
const MAX_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_ENTRIES: usize = 5_000;

#[derive(Debug, Deserialize)]
pub struct Index {
    #[serde(rename = "$schema", default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub skills: Vec<Entry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
}

pub fn index_url(base: &str) -> String {
    format!("{}/.well-known/agent-skills/index.json", base.trim_end_matches('/'))
}

pub fn source_id(base: &str) -> SourceId {
    let host = base.trim_start_matches("https://").trim_start_matches("http://").split('/').next().unwrap_or(base);
    SourceId::new(host, ".well-known/agent-skills")
}

/// Resolve `rel` against `base` per RFC 3986 §5 (absolute, network-path, path-absolute, relative).
pub fn resolve_url(base: &str, rel: &str) -> String {
    if rel.contains("://") {
        return rel.to_string();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    if let Some(net) = rel.strip_prefix("//") {
        return format!("{scheme}://{net}");
    }
    let authority = rest.split('/').next().unwrap_or(rest);
    let base_path = &rest[authority.len()..];
    let merged = if rel.starts_with('/') {
        rel.to_string()
    } else {
        let dir = match base_path.rfind('/') {
            Some(i) => &base_path[..=i],
            None => "/",
        };
        format!("{dir}{rel}")
    };
    // Remove dot segments.
    let mut out: Vec<&str> = Vec::new();
    for seg in merged.split('/') {
        match seg {
            "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut path = out.join("/");
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    format!("{scheme}://{authority}{path}")
}

/// Verify `sha256:<hex>` against the raw bytes.
pub fn verify_digest(bytes: &[u8], digest: &str) -> Result<()> {
    let Some(hex_expected) = digest.strip_prefix("sha256:") else { bail!("unsupported digest `{digest}` (expected sha256:…)") };
    let actual = hex::encode(sha2::Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(hex_expected) {
        bail!("digest mismatch: index says {digest}, content is sha256:{actual}");
    }
    Ok(())
}

fn safe_rel(p: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(x) => out.push(x),
            Component::CurDir => {}
            _ => bail!("archive entry `{}` escapes the skill directory", p.display()),
        }
    }
    Ok(out)
}

/// A link target is allowed only if, resolved from the link's directory, it stays inside the root.
fn link_ok(link_rel: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = link_rel.parent().map(|p| p.components().count() as i64).unwrap_or(0);
    for c in target.components() {
        match c {
            Component::ParentDir => depth -= 1,
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            _ => return false,
        }
        if depth < 0 {
            return false;
        }
    }
    true
}

/// Extract a `.tar.gz` or `.zip` skill archive into `dest` (which must be empty),
/// rejecting traversal, absolute paths and links that resolve outside the skill.
pub fn extract_archive(bytes: &[u8], dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let gz = flate2::read::GzDecoder::new(bytes).take(MAX_ARCHIVE_BYTES);
        let mut ar = tar::Archive::new(gz);
        for (n, entry) in ar.entries()?.enumerate() {
            if n >= MAX_ENTRIES {
                bail!("archive has too many entries");
            }
            let mut entry = entry?;
            let rel = safe_rel(&entry.path()?)?;
            if rel.as_os_str().is_empty() {
                continue;
            }
            let kind = entry.header().entry_type();
            if kind.is_symlink() || kind.is_hard_link() {
                let target = entry.link_name()?.context("link without target")?.to_path_buf();
                let ok = if kind.is_hard_link() { safe_rel(&target).is_ok() } else { link_ok(&rel, &target) };
                if !ok {
                    bail!("archive link `{}` → `{}` resolves outside the skill directory", rel.display(), target.display());
                }
            }
            if !entry.unpack_in(dest)? {
                bail!("archive entry `{}` escapes the skill directory", rel.display());
            }
        }
    } else if bytes.starts_with(b"PK\x03\x04") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
        if zip.len() > MAX_ENTRIES {
            bail!("archive has too many entries");
        }
        let mut total: u64 = 0;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            let Some(rel) = f.enclosed_name() else { bail!("archive entry `{}` escapes the skill directory", f.name()) };
            let rel = safe_rel(&rel)?;
            let out = dest.join(&rel);
            let mode = f.unix_mode();
            let is_link = mode.map(|m| m & 0o170000 == 0o120000).unwrap_or(false);
            if f.is_dir() {
                std::fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut buf = Vec::new();
            (&mut f).take(MAX_ARCHIVE_BYTES - total).read_to_end(&mut buf)?;
            total += buf.len() as u64;
            if total >= MAX_ARCHIVE_BYTES {
                bail!("archive exceeds {} MB", MAX_ARCHIVE_BYTES / 1024 / 1024);
            }
            if is_link {
                let target = PathBuf::from(String::from_utf8_lossy(&buf).to_string());
                if !link_ok(&rel, &target) {
                    bail!("archive link `{}` → `{}` resolves outside the skill directory", rel.display(), target.display());
                }
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target, &out)?;
                continue;
            }
            std::fs::write(&out, &buf)?;
            #[cfg(unix)]
            if let Some(m) = mode {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out, std::fs::Permissions::from_mode(m & 0o777))?;
            }
        }
    } else {
        bail!("unsupported archive format (expected .tar.gz or .zip)");
    }
    if !dest.join("SKILL.md").is_file() {
        bail!("archive has no SKILL.md at its root");
    }
    Ok(())
}

pub fn fetch_index(ctx: &Ctx, base: &str) -> Result<Index> {
    let url = index_url(base);
    let r = ctx.gh.get_public(&url)?;
    if r.status != 200 {
        bail!("{url} returned {}", r.status);
    }
    let idx: Index = serde_json::from_slice(&r.body).with_context(|| format!("parsing {url}"))?;
    match idx.schema.as_deref() {
        Some(SCHEMA_V02) => {}
        Some(other) => bail!("{url}: unrecognized $schema `{other}` (supported: {SCHEMA_V02})"),
        None => {
            if idx.skills.iter().any(|s| s.url.is_none()) {
                bail!("{url}: v0.1.0 index (no $schema, entries without url) is not supported");
            }
            ctx.ui.warn(&format!("{url} has no $schema; treating entries as v0.2.0"));
        }
    }
    Ok(idx)
}

/// Download one entry, verify its digest and materialize it into a temporary directory.
pub fn materialize(ctx: &Ctx, base: &str, e: &Entry) -> Result<(tempfile::TempDir, String, String)> {
    let rel = e.url.as_deref().context("entry has no url")?;
    let url = resolve_url(&index_url(base), rel);
    let digest = e.digest.as_deref().context("entry has no digest (required by the discovery RFC)")?;
    let r = ctx.gh.get_public(&url)?;
    if r.status != 200 {
        bail!("{url} returned {}", r.status);
    }
    verify_digest(&r.body, digest)?;
    let tmp = tempfile::tempdir()?;
    match e.kind.as_deref().unwrap_or("skill-md") {
        "skill-md" => std::fs::write(tmp.path().join("SKILL.md"), &r.body)?,
        "archive" => extract_archive(&r.body, tmp.path())?,
        other => bail!("unsupported entry type `{other}`"),
    }
    Ok((tmp, url, digest.to_string()))
}

pub fn index(ctx: &Ctx, base: &str) -> Result<usize> {
    let base = base.trim_end_matches('/');
    let idx = fetch_index(ctx, base)?;
    let src = source_id(base);
    let mut keep = BTreeSet::new();
    for e in idx.skills {
        let (dir, url, digest) = match materialize(ctx, base, &e) {
            Ok(x) => x,
            Err(err) => {
                ctx.ui.warn(&format!("{base}: skipping `{}`: {err:#}", e.name));
                continue;
            }
        };
        let text = std::fs::read_to_string(dir.path().join("SKILL.md")).unwrap_or_default();
        let files: Vec<(String, bool)> = walkdir::WalkDir::new(dir.path())
            .into_iter()
            .flatten()
            .filter(|f| f.file_type().is_file())
            .map(|f| (f.path().strip_prefix(dir.path()).unwrap().to_string_lossy().replace('\\', "/"), crate::risk::is_exec(f.path())))
            .collect();
        let tree = crate::treehash::tree_hash(dir.path())?;
        let mut rec = IndexedSkill::from_content(&src, &e.name, &text, &files, tree, Some(digest), None, None, None, base);
        if rec.description.is_empty() {
            rec.description = e.description.clone().unwrap_or_default();
        }
        rec.kind = if e.kind.as_deref() == Some("archive") { "wellknown-archive".into() } else { "wellknown".into() };
        rec.url = Some(url);
        keep.insert(rec.id.clone());
        index::upsert(ctx, &rec)?;
    }
    index::prune_source(ctx, &src.to_string(), &keep)?;
    Ok(keep.len())
}

/// The origin (site base URL) and index entry for an installed well-known skill.
pub fn lookup(ctx: &Ctx, id: &str) -> Result<(String, Entry)> {
    let row: Option<(String, Option<String>, Option<String>, String)> = ctx
        .state
        .conn
        .query_row("SELECT origin, url, commit_sha, kind FROM skills WHERE id=?1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .ok();
    let (origin, url, digest, kind) =
        row.with_context(|| format!("{id} is not in the index; add its site with `tricks catalog add <url> --kind wellknown`"))?;
    let name = id.rsplit("//").next().unwrap_or(id).to_string();
    Ok((
        origin,
        Entry { name, kind: Some(if kind == "wellknown-archive" { "archive" } else { "skill-md" }.into()), description: None, url, digest },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_urls() {
        let base = "https://example.com/.well-known/agent-skills/index.json";
        assert_eq!(resolve_url(base, "/.well-known/agent-skills/a/SKILL.md"), "https://example.com/.well-known/agent-skills/a/SKILL.md");
        assert_eq!(resolve_url(base, "a/skill.tar.gz"), "https://example.com/.well-known/agent-skills/a/skill.tar.gz");
        assert_eq!(resolve_url(base, "../../x.zip"), "https://example.com/x.zip");
        assert_eq!(resolve_url(base, "//cdn.example.net/s.zip"), "https://cdn.example.net/s.zip");
        assert_eq!(resolve_url(base, "https://other.dev/s.md"), "https://other.dev/s.md");
    }

    #[test]
    fn digests() {
        let d = format!("sha256:{}", hex::encode(sha2::Sha256::digest(b"hi")));
        assert!(verify_digest(b"hi", &d).is_ok());
        assert!(verify_digest(b"ho", &d).is_err());
        assert!(verify_digest(b"hi", "md5:abc").is_err());
    }

    fn tar_gz(entries: &[(&str, &[u8])], link: Option<(&str, &str)>) -> Vec<u8> {
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
        for (p, c) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(c.len() as u64);
            h.set_mode(0o644);
            // Write the raw name so traversal entries can be constructed for the test.
            h.as_gnu_mut().unwrap().name[..p.len()].copy_from_slice(p.as_bytes());
            h.set_cksum();
            b.append(&h, *c).unwrap();
        }
        if let Some((name, target)) = link {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_size(0);
            b.append_link(&mut h, name, target).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn extracts_and_rejects() {
        let ok = tar_gz(&[("SKILL.md", b"---\nname: a\n---\n"), ("scripts/x.sh", b"echo")], Some(("ref", "scripts/x.sh")));
        let t = tempfile::tempdir().unwrap();
        extract_archive(&ok, &t.path().join("a")).unwrap();
        assert!(t.path().join("a/scripts/x.sh").exists());

        let evil = tar_gz(&[("SKILL.md", b"x"), ("../escape.txt", b"x")], None);
        assert!(extract_archive(&evil, &t.path().join("b")).is_err());
        assert!(!t.path().join("escape.txt").exists());

        let evil_link = tar_gz(&[("SKILL.md", b"x")], Some(("out", "../../etc/passwd")));
        assert!(extract_archive(&evil_link, &t.path().join("c")).is_err());

        let no_skill = tar_gz(&[("README.md", b"x")], None);
        assert!(extract_archive(&no_skill, &t.path().join("d")).is_err());

        let mut zbuf = std::io::Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut zbuf);
            let o: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            z.start_file("SKILL.md", o).unwrap();
            std::io::Write::write_all(&mut z, b"---\nname: z\n---\n").unwrap();
            z.start_file("references/r.md", o).unwrap();
            std::io::Write::write_all(&mut z, b"r").unwrap();
            z.finish().unwrap();
        }
        extract_archive(&zbuf.into_inner(), &t.path().join("e")).unwrap();
        assert!(t.path().join("e/references/r.md").exists());
        assert!(extract_archive(b"not an archive", &t.path().join("f")).is_err());
    }
}
