//! Catalog-hosted skills (not in git): `.well-known/agent-skills` sites and native
//! ClawHub skills. One interface for fetch-and-verify, store snapshots and version labels.

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

/// A hosted skill fetched and verified into the store.
pub struct Stored {
    pub dir: std::path::PathBuf,
    pub tree: String,
    pub commit: String,
    pub label: String,
    pub ref_kind: &'static str,
}

pub enum StoreOutcome {
    Stored(Stored),
    /// Hosted by GitHub after all: use this git skill id instead.
    Redirect(String),
}

/// Fetch (latest, or `version` for ClawHub), verify and add to the store.
pub fn fetch_to_store(ctx: &Ctx, id: &SkillId, version: Option<&str>) -> Result<StoreOutcome> {
    Ok(match fetch(ctx, id, version)? {
        Outcome::Fetched(f) => {
            let (dir, tree) = crate::store::from_dir(ctx, f.dir.path())?;
            StoreOutcome::Stored(Stored { dir, tree, commit: f.commit, label: f.label, ref_kind: f.ref_kind })
        }
        Outcome::Redirect(git_id) => StoreOutcome::Redirect(git_id),
    })
}

/// Re-read the catalog that lists a hosted skill when it is stale, so a new published
/// revision is seen (`.well-known` sites are indexed; ClawHub is asked directly).
pub fn refresh_listing(ctx: &Ctx, id: &SkillId) -> Result<()> {
    if kind(id) == Some(Kind::WellKnown) {
        let (origin, _) = crate::wellknown::lookup(ctx, &id.to_string())?;
        crate::catalogs::refresh(ctx, false, Some(&origin))?;
    }
    Ok(())
}

/// Re-fetch exactly the revision recorded as `commit`, when the catalog can serve it
/// again (ClawHub versions are immutable; `.well-known` indexes only serve the latest).
pub fn refetch(ctx: &Ctx, id: &SkillId, commit: &str) -> Result<Option<Stored>> {
    match (kind(id), commit.strip_prefix("clawhub:")) {
        (Some(Kind::ClawHub), Some(v)) => match fetch_to_store(ctx, id, Some(v))? {
            StoreOutcome::Stored(s) => Ok(Some(s)),
            StoreOutcome::Redirect(_) => Ok(None),
        },
        _ => Ok(None),
    }
}

/// The latest revision the index knows for a hosted skill (no network).
pub fn indexed_commit(ctx: &Ctx, id: &SkillId) -> Option<String> {
    use rusqlite::OptionalExtension;
    ctx.state
        .conn
        .query_row("SELECT commit_sha FROM skills WHERE id=?1", [id.to_string()], |r| r.get::<_, Option<String>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten()
}

/// Human label for a hosted revision: ClawHub version or a short digest.
pub fn label(commit: &str) -> String {
    match commit.strip_prefix("clawhub:") {
        Some(v) => v.to_string(),
        None => commit.trim_start_matches("sha256:").chars().take(12).collect(),
    }
}
