//! `show` and preview: read any skill (remote or local) without executing anything.

use crate::config::LicenseRecord;
use crate::ctx::Ctx;
use crate::license::{self, LicenseInputs};
use crate::resolve::{Fetch, ResolvedSkill, resolve_skill};
use crate::risk::RiskReport;
use crate::skill::SkillDoc;
use crate::store;
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::Serialize;
use std::path::Path;

/// Licence detection for a resolved upstream skill materialized at `dir`.
pub fn detect_license_in_dir(ctx: &Ctx, r: &ResolvedSkill, dir: &Path) -> LicenseRecord {
    let mut inputs = gather_dir(dir);
    let root_names = ["LICENSE", "LICENSE.md", "LICENSE.txt", "LICENCE", "COPYING", "LICENSE-MIT", "LICENSE-APACHE"];
    if r.id.path != "." {
        inputs.repo_root = root_names
            .iter()
            .find_map(|n| r.mirror.read_file(&r.reference.commit, n).ok())
            .map(|b| String::from_utf8_lossy(&b).to_string());
    }
    inputs.api_spdx = ctx
        .state
        .conn
        .query_row("SELECT license FROM repo_info WHERE source=?1", [r.id.source.to_string()], |row| row.get::<_, Option<String>>(0))
        .optional()
        .ok()
        .flatten()
        .flatten();
    license::detect(&inputs)
}

/// Licence inputs found directly in a skill directory.
pub fn gather_dir(dir: &Path) -> LicenseInputs {
    let mut inputs = LicenseInputs::default();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if license::is_license_file(&name)
                && e.path().is_file()
                && let Ok(t) = std::fs::read_to_string(e.path())
            {
                inputs.skill_files.push((name, t));
            }
        }
    }
    if let Ok(t) = std::fs::read_to_string(dir.join("SKILL.md")) {
        inputs.frontmatter = SkillDoc::parse(&t).license;
    }
    inputs
}

#[derive(Debug, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub script: bool,
}

#[derive(Debug, Serialize)]
pub struct ShowReport {
    pub id: String,
    pub canonical: String,
    pub name: String,
    pub description: Option<String>,
    pub commit: String,
    pub ref_kind: String,
    pub ref_name: String,
    pub tree: String,
    pub frontmatter: Option<String>,
    pub frontmatter_error: Option<String>,
    pub body: String,
    pub files: Vec<FileEntry>,
    pub license: LicenseRecord,
    pub risk: RiskReport,
    pub risk_summary: Vec<String>,
    pub listed_in: Vec<String>,
    pub installs: Option<i64>,
    pub trust: String,
    pub installed: bool,
    pub vendored: bool,
    pub store_path: String,
}

pub fn list_files(dir: &Path) -> Vec<FileEntry> {
    let mut v: Vec<FileEntry> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_entry(|e| e.file_name() != ".git")
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let rel = e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
            let exec = crate::risk::is_exec(e.path());
            FileEntry { script: crate::risk::is_script(&rel, exec), size: e.metadata().map(|m| m.len()).unwrap_or(0), path: rel }
        })
        .collect();
    v.sort_by(|a, b| a.path.cmp(&b.path));
    v
}

pub fn show(ctx: &Ctx, input: &str) -> Result<ShowReport> {
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    let r = resolve_skill(ctx, &spec, Fetch::IfStale)?;
    let dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
    let doc = SkillDoc::parse(&text);
    let risk = RiskReport::scan_dir(&dir);
    let license = detect_license_in_dir(ctx, &r, &dir);
    let id = r.id.to_string();
    let mut listed_in = Vec::new();
    let mut installs = None;
    {
        let mut st = ctx.state.conn.prepare("SELECT catalog, installs FROM listings WHERE skill_id=?1")?;
        for row in st.query_map([&id], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))? {
            let (c, i) = row?;
            listed_in.push(c);
            if i.is_some() {
                installs = i;
            }
        }
    }
    let identity = crate::sources::cached_identity(ctx, &r.id.source.host);
    Ok(ShowReport {
        canonical: r.canonical(),
        name: r.name.clone(),
        description: doc.description.clone(),
        commit: r.reference.commit.clone(),
        ref_kind: r.reference.kind.clone(),
        ref_name: r.reference.name.clone(),
        tree: r.tree.clone(),
        frontmatter: doc.frontmatter.clone(),
        frontmatter_error: doc.parse_error.clone(),
        body: doc.body.clone(),
        files: list_files(&dir),
        risk_summary: risk.summary(),
        risk,
        license,
        listed_in,
        installs,
        trust: crate::index::trust_for(r.id.source.owner(), &identity).into(),
        installed: crate::workbench::installed_ids(ctx)?.contains(&id),
        vendored: crate::workspace::vendored_upstreams(ctx)?.contains(&id),
        store_path: dir.to_string_lossy().to_string(),
        id,
    })
}

/// Read one file of a (possibly remote) skill for preview. Never executes anything.
pub fn read_file(ctx: &Ctx, input: &str, rel: &str) -> Result<(String, Vec<u8>)> {
    let rel = rel.trim_start_matches('/');
    if rel.split('/').any(|s| s == "..") {
        anyhow::bail!("invalid path `{rel}`");
    }
    let spec = crate::lookup::spec_from_input(ctx, input)?;
    let r = resolve_skill(ctx, &spec, Fetch::Never).or_else(|_| resolve_skill(ctx, &spec, Fetch::IfStale))?;
    let dir = store::from_mirror(ctx, &r.mirror, &r.reference.commit, &r.id.path, &r.tree)?;
    let p = dir.join(rel);
    let bytes = std::fs::read(&p).with_context(|| format!("{rel} not found in {}", r.canonical()))?;
    Ok((r.canonical(), bytes))
}
