//! Catalog-hosted skills (not in git): `.well-known/agent-skills` sites and native
//! ClawHub skills. One interface for fetch-and-verify, version labels and update checks.

use crate::config::LockedSkill;
use crate::ctx::Ctx;
use crate::id::SkillId;
use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    WellKnown,
    ClawHub,
}

pub fn kind(id: &SkillId) -> Option<Kind> {
    if id.source.repo_path == ".well-known/agent-skills" {
        Some(Kind::WellKnown)
    } else if crate::clawhub::is_clawhub(id) {
        Some(Kind::ClawHub)
    } else {
        None
    }
}

/// Licence a catalog applies to every skill it hosts.
pub fn terms(id: &SkillId) -> Option<&'static str> {
    match kind(id)? {
        Kind::ClawHub => Some(crate::clawhub::TERMS),
        Kind::WellKnown => None,
    }
}

/// Licence of a fetched hosted skill: its own files/frontmatter, else catalog terms.
pub fn license(id: &SkillId, dir: &std::path::Path) -> crate::config::LicenseRecord {
    let mut inputs = crate::inspect::gather_dir(dir);
    inputs.catalog_terms = terms(id).map(String::from);
    crate::license::detect(&inputs)
}

pub struct Fetched {
    pub dir: tempfile::TempDir,
    /// Lock `commit`: `sha256:…` digest (well-known) or `clawhub:<version>`.
    pub commit: String,
    /// Human label: short digest or version.
    pub label: String,
    pub ref_kind: &'static str,
}

pub enum Outcome {
    Fetched(Fetched),
    /// The catalog hands off to a GitHub-hosted source: install it as a git skill.
    Redirect(String),
}

/// Download and verify a hosted skill (latest, or `version` for ClawHub).
pub fn fetch(ctx: &Ctx, id: &SkillId, version: Option<&str>) -> Result<Outcome> {
    match kind(id) {
        Some(Kind::WellKnown) => {
            let (origin, entry) = crate::wellknown::lookup(ctx, &id.to_string())?;
            let (dir, _url, digest) = crate::wellknown::materialize(ctx, &origin, &entry)?;
            let label = digest.trim_start_matches("sha256:").chars().take(12).collect();
            Ok(Outcome::Fetched(Fetched { dir, commit: digest, label, ref_kind: "digest" }))
        }
        Some(Kind::ClawHub) => {
            let (owner, slug) = crate::clawhub::owner_slug(id)?;
            match crate::clawhub::download(ctx, &owner, &slug, version)? {
                crate::clawhub::Download::Skill { dir, version } => {
                    Ok(Outcome::Fetched(Fetched { dir, commit: format!("clawhub:{version}"), label: version, ref_kind: "version" }))
                }
                crate::clawhub::Download::GitHub { repo, path, .. } => {
                    let src = crate::id::parse_source_input(&repo).or_else(|_| crate::id::SourceId::parse(&repo))?;
                    Ok(Outcome::Redirect(format!("{src}//{}", if path.is_empty() { "." } else { path.trim_matches('/') })))
                }
            }
        }
        None => bail!("{id} is not a catalog-hosted skill"),
    }
}

/// A newer published version than the locked one: (label, lock commit).
pub fn newer(ctx: &Ctx, locked: &LockedSkill, refresh: bool) -> Result<Option<(String, String)>> {
    let id = SkillId::parse_canonical(&locked.id)?;
    match kind(&id) {
        Some(Kind::WellKnown) => {
            if refresh {
                let (origin, _) = crate::wellknown::lookup(ctx, &locked.id)?;
                let _ = crate::sources::refresh(ctx, false, Some(&origin));
            }
            let (_, entry) = crate::wellknown::lookup(ctx, &locked.id)?;
            Ok(entry.digest.filter(|d| d != &locked.commit).map(|d| (d.trim_start_matches("sha256:").chars().take(12).collect(), d)))
        }
        Some(Kind::ClawHub) => {
            if ctx.opts.offline {
                return Ok(None);
            }
            let (owner, slug) = crate::clawhub::owner_slug(&id)?;
            let v = crate::clawhub::latest_version(ctx, &owner, &slug)?;
            Ok((v != locked.ref_name).then(|| (v.clone(), format!("clawhub:{v}"))))
        }
        None => Ok(None),
    }
}
