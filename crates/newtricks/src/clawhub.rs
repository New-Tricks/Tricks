//! ClawHub adapter (live query). Two kinds of results:
//! - mirrors of GitHub-hosted skills (e.g. from skills.sh) become git pointers;
//! - native ClawHub skills get the id `clawhub.ai/<owner>/skills//<slug>` (mirroring
//!   ClawHub's `/<owner>/skills/<slug>` URLs) and install from ClawHub's ZIP download,
//!   verified file by file against the version's published SHA-256 list.

use crate::ctx::Ctx;
use crate::github::{GitHub, encode_query};
use crate::id::{SkillId, SourceId};
use crate::index::{self, IndexedSkill};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::Digest;

pub const CATALOG: &str = "clawhub";
pub const HOST: &str = "clawhub.ai";
const MAX_NATIVE: usize = 8;
/// "All skills published on ClawHub are licensed under MIT-0" (ClawHub docs/skill-format.md).
pub const TERMS: &str = "MIT-0";

fn base_url() -> String {
    std::env::var("TRICKS_CLAWHUB_URL").unwrap_or_else(|_| "https://clawhub.ai".into())
}

pub fn is_clawhub(id: &SkillId) -> bool {
    id.source.host == HOST && id.source.repo_path.ends_with("/skills")
}

pub fn id_for(owner: &str, slug: &str) -> SkillId {
    SkillId::new(SourceId::new(HOST, &format!("{owner}/skills")), slug)
}

/// (owner, slug) of a ClawHub skill id.
pub fn owner_slug(id: &SkillId) -> Result<(String, String)> {
    if !is_clawhub(id) {
        bail!("{id} is not a ClawHub skill");
    }
    Ok((id.source.owner().to_string(), id.path.clone()))
}

fn get_json(gh: &GitHub, path: &str) -> Result<Value> {
    let url = format!("{}{path}", base_url());
    let r = gh.get_public(&url)?;
    if r.status != 200 {
        bail!("ClawHub {path} returned {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().take(160).collect::<String>());
    }
    Ok(serde_json::from_slice(&r.body)?)
}

pub struct Detail {
    pub skill_md: String,
    pub version: String,
    pub files: Vec<(String, String)>,
    pub signals: Value,
    pub updated_at: Option<i64>,
    pub installs: Option<i64>,
}

fn latest(d: &Value) -> Option<String> {
    d["latestVersion"]["version"].as_str().or_else(|| d["skill"]["tags"]["latest"].as_str()).map(String::from)
}

/// Skill detail + latest (or given) version detail. `None` when no version is given and
/// none is published: a GitHub-backed skill, which ClawHub hands off on download.
pub fn detail(gh: &GitHub, owner: &str, slug: &str, version: Option<&str>) -> Result<Option<Detail>> {
    let q = format!("owner={}", encode_query(owner));
    let d = get_json(gh, &format!("/api/v1/skills/{}?{q}", encode_query(slug)))?;
    let skill = &d["skill"];
    let version = match version.map(String::from).or_else(|| latest(&d)) {
        Some(v) => v,
        None => return Ok(None),
    };
    let v = get_json(gh, &format!("/api/v1/skills/{}/versions/{}?{q}", encode_query(slug), encode_query(&version)))?;
    let ver = &v["version"];
    let files: Vec<(String, String)> = ver["files"]
        .as_array()
        .map(|a| a.iter().filter_map(|f| Some((f["path"].as_str()?.to_string(), f["sha256"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    let sec = &ver["security"];
    let vt = sec["scanners"]["vt"]["verdict"].as_str().map(String::from);
    let signals = serde_json::json!({
        "installs": skill["stats"]["installs"],
        "downloads": skill["stats"]["downloads"],
        "stars": skill["stats"]["stars"],
        "suspicious": d["moderation"]["isSuspicious"].as_bool().or(skill["isSuspicious"].as_bool()).unwrap_or(false),
        "security_status": sec["status"],
        "scanner_warnings": sec["hasWarnings"],
        "virustotal": vt,
        "moderation": d["moderation"]["verdict"],
        "malware_blocked": d["moderation"]["isMalwareBlocked"].as_bool().unwrap_or(false),
        "version": version,
    });
    Ok(Some(Detail {
        skill_md: skill["description"].as_str().unwrap_or_default().to_string(),
        version,
        files,
        signals,
        updated_at: skill["updatedAt"].as_i64().map(|ms| ms / 1000),
        installs: skill["stats"]["installs"].as_i64(),
    }))
}

/// Index a native ClawHub skill from its detail (no download needed).
fn index_native(ctx: &Ctx, owner: &str, slug: &str, d: Detail) -> Result<String> {
    let id = id_for(owner, slug);
    let files: Vec<(String, bool)> = d.files.iter().map(|(p, _)| (p.clone(), false)).collect();
    let mut rec = IndexedSkill::from_content(
        &id.source,
        &id.path,
        &d.skill_md,
        &files,
        None,
        Some(format!("clawhub:{}", d.version)),
        Some(d.version.clone()),
        d.updated_at,
        Some(TERMS),
        CATALOG,
    );
    rec.kind = "clawhub".into();
    rec.url = Some(format!("https://{HOST}/{owner}/skills/{slug}"));
    index::upsert(ctx, &rec)?;
    index::add_listing_signals(ctx, &rec.id, CATALOG, d.installs, &d.signals)?;
    Ok(rec.id)
}

/// Live search: mirrors become git pointers; native skills are indexed from their detail.
pub fn live_search(ctx: &Ctx, q: &str, limit: usize) -> Result<usize> {
    if ctx.opts.offline || q.trim().is_empty() || crate::sources::live_fresh(ctx, CATALOG, q)? {
        return Ok(0);
    }
    // Skills ClawHub flags as suspicious are excluded from search.
    let resp = get_json(&ctx.gh, &format!("/api/v1/search?q={}&limit={limit}&nonSuspiciousOnly=true", encode_query(q)))?;
    let results = resp["results"].as_array().cloned().unwrap_or_default();
    let mut repos: Vec<SourceId> = Vec::new();
    let mut mirrors: Vec<(SourceId, String, Option<i64>)> = Vec::new();
    let mut natives: Vec<(String, String)> = Vec::new();
    for r in &results {
        let si = &r["sourceIdentity"];
        let kind = r["install"]["kind"].as_str().unwrap_or("");
        if let (Some(owner), Some(repo)) = (si["owner"].as_str(), si["repo"].as_str()) {
            let src = SourceId::new(si["host"].as_str().unwrap_or("github.com"), &format!("{owner}/{repo}"));
            let name = si["id"].as_str().and_then(|i| i.rsplit('/').next()).or(r["slug"].as_str()).unwrap_or_default().to_string();
            if !repos.contains(&src) {
                repos.push(src.clone());
            }
            mirrors.push((src, name, si["lifetimeInstalls"].as_i64()));
        } else if kind == "clawhub"
            && let (Some(owner), Some(slug)) = (r["ownerHandle"].as_str(), r["slug"].as_str())
            && natives.len() < MAX_NATIVE
        {
            natives.push((owner.to_string(), slug.to_string()));
        }
    }
    repos.truncate(6);
    // ClawHub's API is slow (seconds per call) and a native skill needs two calls, so
    // fetch details in parallel while the mirrored repositories are indexed.
    let gh = &ctx.gh;
    let details = std::thread::scope(|s| {
        let h = s.spawn(|| {
            crate::sources::parallel_map(natives, MAX_NATIVE, |(o, sl)| {
                let d = detail(gh, &o, &sl, None);
                (o, sl, d)
            })
        });
        crate::sources::ensure_repos_indexed(ctx, &repos);
        h.join().unwrap_or_default()
    });
    let mut n = 0;
    for (src, name, installs) in mirrors {
        if let Some(id) = index::find_in_source(ctx, &src.to_string(), &name)? {
            index::add_listing(ctx, &id, CATALOG, installs, None)?;
            n += 1;
        }
    }
    for (owner, slug, d) in details {
        match d {
            Ok(Some(d)) => match index_native(ctx, &owner, &slug, d) {
                Ok(_) => n += 1,
                Err(e) => ctx.ui.warn(&format!("ClawHub {owner}/{slug}: {e:#}")),
            },
            Ok(None) => {} // GitHub-backed: resolved to its repository on install
            Err(e) => ctx.ui.warn(&format!("ClawHub {owner}/{slug}: {e:#}")),
        }
    }
    crate::sources::live_mark(ctx, CATALOG, q)?;
    Ok(n)
}

pub enum Download {
    /// Extracted and verified skill directory.
    Skill { dir: tempfile::TempDir, version: String },
    /// ClawHub points at a GitHub-hosted source instead of hosting a ZIP.
    GitHub { repo: String, path: String, commit: String },
}

/// Download a native skill version and verify every file against the published SHA-256.
pub fn download(ctx: &Ctx, owner: &str, slug: &str, version: Option<&str>) -> Result<Download> {
    let d = detail(&ctx.gh, owner, slug, version)?;
    let mut url = format!("{}/api/v1/download?slug={}&owner={}", base_url(), encode_query(slug), encode_query(owner));
    if let Some(d) = &d {
        url.push_str(&format!("&version={}", encode_query(&d.version)));
    }
    let r = ctx.gh.get_public(&url)?;
    if r.status != 200 {
        bail!("ClawHub download returned {}: {}", r.status, String::from_utf8_lossy(&r.body).chars().take(160).collect::<String>());
    }
    if r.body.first() == Some(&b'{') {
        let h: Value = serde_json::from_slice(&r.body)?;
        return Ok(Download::GitHub {
            repo: h["repo"].as_str().context("handoff without repo")?.to_string(),
            path: h["path"].as_str().unwrap_or(".").to_string(),
            commit: h["commit"].as_str().unwrap_or_default().to_string(),
        });
    }
    let d = d.with_context(|| format!("{owner}/{slug} has no published version"))?;
    let tmp = tempfile::tempdir()?;
    crate::wellknown::extract_archive(&r.body, tmp.path())?;
    if d.files.is_empty() {
        bail!("ClawHub published no file hashes for {owner}/{slug}@{}; refusing an unverifiable download", d.version);
    }
    let expected: std::collections::BTreeMap<String, String> = d.files.into_iter().collect();
    for e in walkdir::WalkDir::new(tmp.path()).into_iter().flatten() {
        if !e.file_type().is_file() {
            continue;
        }
        let rel = e.path().strip_prefix(tmp.path()).unwrap().to_string_lossy().replace('\\', "/");
        if rel == "_meta.json" {
            std::fs::remove_file(e.path())?; // registry bookkeeping, not part of the skill
            continue;
        }
        let Some(want) = expected.get(&rel) else { bail!("ClawHub archive contains unlisted file `{rel}`") };
        let got = hex::encode(sha2::Sha256::digest(std::fs::read(e.path())?));
        if !got.eq_ignore_ascii_case(want) {
            bail!("ClawHub file `{rel}` does not match its published SHA-256");
        }
    }
    for p in expected.keys() {
        if !tmp.path().join(p).is_file() {
            bail!("ClawHub archive is missing `{p}`");
        }
    }
    Ok(Download::Skill { dir: tmp, version: d.version })
}

/// Latest published version of a native skill.
pub fn latest_version(ctx: &Ctx, owner: &str, slug: &str) -> Result<String> {
    let d = get_json(&ctx.gh, &format!("/api/v1/skills/{}?owner={}", encode_query(slug), encode_query(owner)))?;
    latest(&d).context("no published version")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        let id = id_for("awspace", "pdf");
        assert_eq!(id.to_string(), "clawhub.ai/awspace/skills//pdf");
        assert!(is_clawhub(&id));
        assert_eq!(owner_slug(&id).unwrap(), ("awspace".into(), "pdf".into()));
        let parsed = crate::id::SkillSpec::parse("clawhub.ai/awspace/skills//pdf@0.1.0").unwrap();
        assert!(is_clawhub(&SkillId::new(parsed.source, &parsed.selector)));
        assert!(!is_clawhub(&SkillId::parse_canonical("github.com/a/skills//x").unwrap()));
    }
}
