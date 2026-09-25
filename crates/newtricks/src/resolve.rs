//! Ref and skill resolution (spec §5): `@latest` → highest semver tag else default
//! branch; ambiguous short names resolve tag → branch → commit; the part after `//` is
//! an exact path, then a frontmatter name, then a folder name.

use crate::ctx::Ctx;
use crate::git::{Mirror, git};
use crate::id::{SkillId, SkillSpec, SourceId};
use crate::skill::SkillDoc;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedRef {
    /// tag | branch | commit | default-branch
    pub kind: String,
    pub name: String,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedSkill {
    pub id: SkillId,
    pub name: String,
    pub reference: ResolvedRef,
    pub tree: String,
    #[serde(skip)]
    pub mirror: Mirror,
}

impl ResolvedSkill {
    pub fn canonical(&self) -> String {
        self.id.with_ref(&self.reference.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fetch {
    /// Fetch if the mirror is older than the user fetch interval.
    IfStale,
    Always,
    Never,
}

/// Local refs of a mirror: tags (peeled) and heads.
pub struct MirrorRefs {
    pub default_branch: Option<String>,
    pub heads: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
}

pub fn mirror_refs(m: &Mirror) -> Result<MirrorRefs> {
    let out = git(&m.dir, &["for-each-ref", "--format=%(refname)\t%(objectname)\t%(*objectname)", "refs/heads", "refs/tags"])?;
    let mut heads = BTreeMap::new();
    let mut tags = BTreeMap::new();
    for line in out.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 2 {
            continue;
        }
        if let Some(h) = cols[0].strip_prefix("refs/heads/") {
            heads.insert(h.to_string(), cols[1].to_string());
        } else if let Some(t) = cols[0].strip_prefix("refs/tags/") {
            let commit = cols.get(2).filter(|s| !s.is_empty()).unwrap_or(&cols[1]);
            tags.insert(t.to_string(), commit.to_string());
        }
    }
    Ok(MirrorRefs { default_branch: m.default_branch(), heads, tags })
}

pub fn parse_semver(tag: &str) -> Option<semver::Version> {
    let t = tag.strip_prefix('v').or_else(|| tag.strip_prefix('V')).unwrap_or(tag);
    semver::Version::parse(t).ok().or_else(|| {
        // Accept `v1.2` / `v1` as `1.2.0` / `1.0.0`.
        let parts: Vec<&str> = t.split('.').collect();
        if parts.len() < 3 && parts.iter().all(|p| p.parse::<u64>().is_ok()) {
            let mut p = parts.iter().map(|s| s.to_string()).collect::<Vec<_>>();
            while p.len() < 3 {
                p.push("0".into());
            }
            semver::Version::parse(&p.join(".")).ok()
        } else {
            None
        }
    })
}

/// Highest semver tag (stable preferred over pre-release).
pub fn latest_tag(tags: &BTreeMap<String, String>) -> Option<(String, String)> {
    let mut v: Vec<(semver::Version, &String, &String)> = tags.iter().filter_map(|(n, c)| parse_semver(n).map(|s| (s, n, c))).collect();
    if v.iter().any(|(s, _, _)| s.pre.is_empty()) {
        v.retain(|(s, _, _)| s.pre.is_empty());
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.last().map(|(_, n, c)| ((*n).clone(), (*c).clone()))
}

pub fn resolve_ref(m: &Mirror, refs: &MirrorRefs, requested: Option<&str>, warn: &mut Vec<String>) -> Result<ResolvedRef> {
    let req = requested.unwrap_or("latest");
    if req == "latest" {
        if let Some((name, commit)) = latest_tag(&refs.tags) {
            return Ok(ResolvedRef { kind: "tag".into(), name, commit });
        }
        let b = refs.default_branch.clone().or_else(|| refs.heads.keys().next().cloned()).context("repository has no branches")?;
        let commit = refs.heads.get(&b).cloned().context("default branch has no commit")?;
        return Ok(ResolvedRef { kind: "default-branch".into(), name: b, commit });
    }
    if let Some(b) = req.strip_prefix("refs/heads/") {
        let commit = refs.heads.get(b).cloned().with_context(|| format!("no branch `{b}` in {}", m.source))?;
        return Ok(ResolvedRef { kind: "branch".into(), name: b.into(), commit });
    }
    if let Some(t) = req.strip_prefix("refs/tags/") {
        let commit = refs.tags.get(t).cloned().with_context(|| format!("no tag `{t}` in {}", m.source))?;
        return Ok(ResolvedRef { kind: "tag".into(), name: t.into(), commit });
    }
    let tag = refs.tags.get(req);
    let head = refs.heads.get(req);
    if tag.is_some() && head.is_some() {
        warn.push(format!("`{req}` is both a tag and a branch in {}; using the tag (use @refs/heads/{req} for the branch)", m.source));
    }
    if let Some(c) = tag {
        return Ok(ResolvedRef { kind: "tag".into(), name: req.into(), commit: c.clone() });
    }
    if let Some(c) = head {
        return Ok(ResolvedRef { kind: "branch".into(), name: req.into(), commit: c.clone() });
    }
    if req.len() >= 7 && req.chars().all(|c| c.is_ascii_hexdigit()) {
        let commit = match m.rev_parse(req) {
            Ok(c) => c,
            Err(_) => {
                m.ensure_commit(req)?;
                m.rev_parse(req)?
            }
        };
        return Ok(ResolvedRef { kind: "commit".into(), name: commit[..12.min(commit.len())].to_string(), commit });
    }
    bail!("`{req}` is not a tag, branch or commit of {}", m.source)
}

/// Open (clone/fetch) the mirror for a source according to the fetch mode.
pub fn open_mirror(ctx: &Ctx, src: &SourceId, fetch: Fetch) -> Result<Mirror> {
    let key = format!("mirror:{src}");
    let exists = Mirror::path_for(&ctx.paths, src).join("HEAD").exists();
    if ctx.opts.offline {
        if !exists {
            bail!("{src} is not cached and --offline was given");
        }
        return Mirror::open(&ctx.paths, src, false);
    }
    let interval = crate::user::config(ctx)?.fetch_interval();
    let do_fetch = match fetch {
        Fetch::Always => true,
        Fetch::Never => false,
        Fetch::IfStale => ctx.state.is_stale(&key, interval)?,
    };
    if !exists || do_fetch {
        ctx.ui.info(&format!("{} {src}", if exists { "fetching" } else { "cloning" }));
    }
    // Per-source lock: the CLI and the VS Code extension's server may run concurrently.
    let _lock = if !exists || do_fetch { Some(source_lock(ctx, src)?) } else { None };
    let m = Mirror::open(&ctx.paths, src, do_fetch)?;
    if !exists || do_fetch {
        ctx.state.mark_fetched(&key)?;
    }
    Ok(m)
}

/// Exclusive per-source lock held for the lifetime of the returned file.
pub fn source_lock(ctx: &Ctx, src: &SourceId) -> Result<std::fs::File> {
    let name = format!("{}.lock", crate::paths::path_key(std::path::Path::new(&src.to_string())));
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(ctx.paths.locks().join(name))?;
    f.lock().with_context(|| format!("locking {src}"))?;
    Ok(f)
}

/// Read the frontmatter name of the skill at `path` in `commit`.
pub fn skill_name_at(m: &Mirror, commit: &str, path: &str) -> Option<String> {
    let p = if path == "." { "SKILL.md".to_string() } else { format!("{path}/SKILL.md") };
    let bytes = m.read_file(commit, &p).ok()?;
    SkillDoc::parse(&String::from_utf8_lossy(&bytes)).name
}

/// Resolve a user skill spec to a canonical skill at an exact commit.
pub fn resolve_skill(ctx: &Ctx, spec: &SkillSpec, fetch: Fetch) -> Result<ResolvedSkill> {
    let mut mirror = open_mirror(ctx, &spec.source, fetch)?;
    let mut refs = mirror_refs(&mirror)?;
    let mut warns = Vec::new();
    let reference = match resolve_ref(&mirror, &refs, spec.reference.as_deref(), &mut warns) {
        Ok(r) => r,
        Err(e) if fetch != Fetch::Always && !ctx.opts.offline && spec.reference.is_some() => {
            // The mirror may be stale: fetch once and retry.
            mirror = open_mirror(ctx, &spec.source, Fetch::Always)?;
            refs = mirror_refs(&mirror)?;
            resolve_ref(&mirror, &refs, spec.reference.as_deref(), &mut warns).map_err(|_| e)?
        }
        Err(e) => return Err(e),
    };
    for w in warns {
        ctx.ui.warn(&w);
    }
    let path = select_path(&mirror, &reference.commit, &spec.selector)?;
    let tree = mirror.tree_at(&reference.commit, &path).context("skill directory disappeared")?;
    let id = SkillId::new(spec.source.clone(), &path);
    let name = skill_name_at(&mirror, &reference.commit, &path).unwrap_or_else(|| id.folder_name().to_string());
    Ok(ResolvedSkill { id, name, reference, tree, mirror })
}

/// Resolve a canonical id at a requested ref (used for installed skills).
pub fn resolve_id(ctx: &Ctx, id: &SkillId, requested: &str, fetch: Fetch) -> Result<ResolvedSkill> {
    let spec = SkillSpec { source: id.source.clone(), selector: id.path.clone(), reference: Some(requested.to_string()) };
    resolve_skill(ctx, &spec, fetch)
}

pub fn select_path(m: &Mirror, commit: &str, selector: &str) -> Result<String> {
    let direct = if selector == "." { "SKILL.md".to_string() } else { format!("{selector}/SKILL.md") };
    if m.file_exists(commit, &direct) {
        return Ok(selector.to_string());
    }
    let dirs = m.skill_dirs(commit)?;
    if selector.contains('/') || selector == "." {
        bail!("no skill at `{selector}` in {} (no SKILL.md there)", m.source);
    }
    let by_name: Vec<&String> = dirs.iter().map(|(d, _)| d).filter(|d| skill_name_at(m, commit, d).as_deref() == Some(selector)).collect();
    let candidates: Vec<&String> = if !by_name.is_empty() {
        by_name
    } else {
        let folder =
            |d: &str| -> String { if d == "." { m.source.name().to_string() } else { d.rsplit('/').next().unwrap_or(d).to_string() } };
        dirs.iter().map(|(d, _)| d).filter(|d| folder(d) == selector).collect()
    };
    match candidates.len() {
        0 => {
            let names: Vec<String> = dirs.iter().take(30).map(|(d, _)| d.clone()).collect();
            bail!("no skill named `{selector}` in {}. Available: {}", m.source, names.join(", "))
        }
        1 => Ok(candidates[0].clone()),
        _ => bail!(
            "`{selector}` is ambiguous in {}: {}",
            m.source,
            candidates.iter().map(|c| format!("{}//{c}", m.source)).collect::<Vec<_>>().join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_prefers_stable_semver() {
        let mut t = BTreeMap::new();
        t.insert("v1.2.0".to_string(), "a".to_string());
        t.insert("v1.10.0".to_string(), "b".to_string());
        t.insert("v2.0.0-rc.1".to_string(), "c".to_string());
        t.insert("nightly".to_string(), "d".to_string());
        assert_eq!(latest_tag(&t), Some(("v1.10.0".into(), "b".into())));
        let mut only_pre = BTreeMap::new();
        only_pre.insert("v2.0.0-rc.1".to_string(), "c".to_string());
        assert_eq!(latest_tag(&only_pre).unwrap().0, "v2.0.0-rc.1");
    }
}
