//! Local search index (spec §7): FTS5 over real skill content, facets, ranking and
//! grouping of identical copies by tree hash.

use crate::ctx::Ctx;
use crate::license::{Class, classify_expression};
use crate::risk;
use crate::skill::SkillDoc;
use crate::state::now;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const OFFICIAL_OWNERS: &[&str] = &["anthropics", "openai", "github", "microsoft", "vercel-labs", "google", "google-gemini", "cursor"];

#[derive(Debug, Clone, Default)]
pub struct IndexedSkill {
    pub id: String,
    pub source: String,
    pub host: String,
    pub owner: String,
    pub repo: String,
    pub path: String,
    pub name: String,
    pub folder: String,
    pub description: String,
    pub body: String,
    pub frontmatter: String,
    pub license: Option<String>,
    pub license_class: String,
    pub tree: Option<String>,
    pub commit: Option<String>,
    pub ref_name: Option<String>,
    pub updated_at: Option<i64>,
    pub has_scripts: bool,
    pub allowed_tools: Option<String>,
    pub network: bool,
    pub file_count: usize,
    pub compat: String,
    pub kind: String,
    pub url: Option<String>,
    pub origin: String,
}

impl IndexedSkill {
    /// Build a record from SKILL.md text and the skill's file list (path, executable).
    #[allow(clippy::too_many_arguments)]
    pub fn from_content(
        source: &crate::id::SourceId,
        path: &str,
        skill_md: &str,
        files: &[(String, bool)],
        tree: Option<String>,
        commit: Option<String>,
        ref_name: Option<String>,
        updated_at: Option<i64>,
        repo_license: Option<&str>,
        origin: &str,
    ) -> IndexedSkill {
        let doc = SkillDoc::parse(skill_md);
        let id = crate::id::SkillId::new(source.clone(), path);
        let folder = id.folder_name().to_string();
        let has_license_file = files.iter().any(|(f, _)| !f.contains('/') && crate::license::is_license_file(f));
        let license_class = index_license_class(doc.license.as_deref(), has_license_file, repo_license);
        let network = !risk::RiskReport::scan_files(std::iter::once(("SKILL.md", skill_md.as_bytes(), false))).urls.is_empty();
        IndexedSkill {
            id: id.to_string(),
            source: source.to_string(),
            host: source.host.clone(),
            owner: source.owner().to_string(),
            repo: source.repo_path.clone(),
            path: id.path.clone(),
            name: doc.name.clone().unwrap_or_else(|| folder.clone()),
            folder,
            description: doc.description.clone().unwrap_or_default().trim().to_string(),
            body: doc.body.chars().take(20_000).collect(),
            frontmatter: doc.frontmatter.clone().unwrap_or_default(),
            license: doc.license.clone().or_else(|| repo_license.map(String::from)),
            license_class,
            tree,
            commit,
            ref_name,
            updated_at,
            has_scripts: files.iter().any(|(f, x)| risk::is_script(f, *x)),
            allowed_tools: doc.allowed_tools.clone(),
            network,
            file_count: files.len(),
            compat: compat_agents(&doc).join(","),
            kind: "git".into(),
            url: None,
            origin: origin.to_string(),
        }
    }
}

/// Coarse licence class for search facets (full detection happens at show/vendor time).
pub fn index_license_class(frontmatter: Option<&str>, has_file: bool, repo_license: Option<&str>) -> String {
    if let Some(fm) = frontmatter.map(str::trim).filter(|s| !s.is_empty()) {
        let l = fm.to_ascii_lowercase();
        if l.contains("proprietary") || l.contains("all rights reserved") {
            return "block".into();
        }
        if let Some(c) = classify_expression(fm) {
            return c.as_str().into();
        }
        if has_file {
            return "unknown".into();
        }
    }
    if has_file {
        return "unknown".into();
    }
    match repo_license.filter(|s| *s != "NOASSERTION") {
        Some(id) => crate::license::classify_id(id).as_str().into(),
        None => "block".into(),
    }
}

/// Agents a skill targets, from agent-specific keys or the `compatibility` field.
pub fn compat_agents(doc: &SkillDoc) -> Vec<String> {
    let all = ["claude", "codex", "copilot", "cursor"];
    if let Some(c) = &doc.compatibility {
        let l = c.to_ascii_lowercase();
        let hits: Vec<String> = [("claude", "claude"), ("codex", "codex"), ("copilot", "copilot"), ("cursor", "cursor")]
            .iter()
            .filter(|(k, _)| l.contains(k))
            .map(|(_, v)| v.to_string())
            .collect();
        if !hits.is_empty() && !l.contains("or similar") {
            return hits;
        }
    }
    all.iter().map(|s| s.to_string()).collect()
}

pub fn upsert(ctx: &Ctx, s: &IndexedSkill) -> Result<()> {
    let c = &ctx.state.conn;
    c.execute(
        "INSERT OR REPLACE INTO skills(id, source, host, owner, repo, path, name, folder, description, body, frontmatter, license, license_class,
          tree, commit_sha, ref_name, updated_at, indexed_at, has_scripts, allowed_tools, network, file_count, compat, kind, url, origin)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26)",
        params![
            s.id, s.source, s.host, s.owner, s.repo, s.path, s.name, s.folder, s.description, s.body, s.frontmatter, s.license,
            s.license_class, s.tree, s.commit, s.ref_name, s.updated_at, now(), s.has_scripts, s.allowed_tools, s.network,
            s.file_count as i64, s.compat, s.kind, s.url, s.origin
        ],
    )?;
    c.execute("DELETE FROM skills_fts WHERE id=?1", [&s.id])?;
    c.execute(
        "INSERT INTO skills_fts(id, name, description, body) VALUES(?1, ?2, ?3, ?4)",
        params![s.id, format!("{} {}", s.name, s.folder), s.description, s.body],
    )?;
    Ok(())
}

/// Remove indexed skills of `source` not in `keep`.
pub fn prune_source(ctx: &Ctx, source: &str, keep: &BTreeSet<String>) -> Result<usize> {
    let c = &ctx.state.conn;
    let mut st = c.prepare("SELECT id FROM skills WHERE source=?1")?;
    let ids: Vec<String> = st.query_map([source], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut n = 0;
    for id in ids.into_iter().filter(|i| !keep.contains(i)) {
        c.execute("DELETE FROM skills WHERE id=?1", [&id])?;
        c.execute("DELETE FROM skills_fts WHERE id=?1", [&id])?;
        n += 1;
    }
    Ok(n)
}

pub fn add_listing(ctx: &Ctx, skill_id: &str, catalog: &str, installs: Option<i64>, category: Option<&str>) -> Result<()> {
    ctx.state.conn.execute(
        "INSERT INTO listings(skill_id, catalog, installs, category, at) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(skill_id, catalog) DO UPDATE SET installs=COALESCE(excluded.installs, installs), category=COALESCE(excluded.category, category), at=excluded.at",
        params![skill_id, catalog, installs, category, now()],
    )?;
    Ok(())
}

pub fn skill_exists(ctx: &Ctx, id: &str) -> Result<bool> {
    Ok(ctx.state.conn.query_row("SELECT 1 FROM skills WHERE id=?1", [id], |_| Ok(())).optional()?.is_some())
}

/// Find an indexed skill in `source` by frontmatter name or folder.
pub fn find_in_source(ctx: &Ctx, source: &str, name: &str) -> Result<Option<String>> {
    Ok(ctx
        .state
        .conn
        .query_row(
            "SELECT id FROM skills WHERE source=?1 AND (name=?2 OR folder=?2) ORDER BY (name=?2) DESC LIMIT 1",
            params![source, name],
            |r| r.get(0),
        )
        .optional()?)
}

#[derive(Debug, Clone, Default)]
pub struct Filters {
    pub agent: Option<String>,
    pub trust: Option<String>,
    pub license: Option<String>,
    pub source: Option<String>,
    pub owner: Option<String>,
    pub installed: bool,
    pub no_scripts: bool,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub path: String,
    pub tree: Option<String>,
    pub commit: Option<String>,
    pub ref_name: Option<String>,
    pub license: Option<String>,
    pub license_class: String,
    pub trust: String,
    pub installs: Option<i64>,
    pub stars: Option<i64>,
    pub listed_in: Vec<String>,
    pub categories: Vec<String>,
    pub updated_at: Option<i64>,
    pub risk: Vec<String>,
    pub agents: Vec<String>,
    pub installed: bool,
    pub vendored: bool,
    /// Other ids with identical content (same tree hash).
    pub duplicates: Vec<String>,
    /// Indexed skills with the same name but different content.
    pub variants: usize,
    pub kind: String,
    pub score: f64,
}

/// Trust facet: official · yours · org · unknown.
pub fn trust_for(owner: &str, identity: &Option<(String, Vec<String>)>) -> &'static str {
    if let Some((login, orgs)) = identity {
        if owner.eq_ignore_ascii_case(login) {
            return "yours";
        }
        if orgs.iter().any(|o| o.eq_ignore_ascii_case(owner)) {
            return "org";
        }
    }
    if OFFICIAL_OWNERS.iter().any(|o| o.eq_ignore_ascii_case(owner)) {
        return "official";
    }
    "unknown"
}

/// Build an FTS5 query from free text: every term must match (prefix match on the last).
pub fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> =
        q.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_').filter(|t| !t.is_empty()).map(|t| t.replace('"', "")).collect();
    if terms.is_empty() {
        return None;
    }
    let n = terms.len();
    Some(
        terms
            .iter()
            .enumerate()
            .map(|(i, t)| if i + 1 == n { format!("\"{t}\"*") } else { format!("\"{t}\"") })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

pub fn search(ctx: &Ctx, query: &str, f: &Filters, limit: usize) -> Result<Vec<SearchResult>> {
    let identity = crate::sources::cached_identity(ctx, "github.com");
    let installed: BTreeSet<String> = crate::workbench::installed_ids(ctx).unwrap_or_default();
    let vendored: BTreeSet<String> = crate::workspace::vendored_upstreams(ctx).unwrap_or_default();
    let c = &ctx.state.conn;

    let mut rows: Vec<(String, f64)> = Vec::new();
    match fts_query(query) {
        Some(fq) => {
            let mut st =
                c.prepare("SELECT id, bm25(skills_fts, 0.0, 10.0, 4.0, 1.0) FROM skills_fts WHERE skills_fts MATCH ?1 LIMIT 2000")?;
            let it = st.query_map([&fq], |r| Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?)))?;
            for r in it {
                let (id, bm) = r?;
                rows.push((id, -bm));
            }
        }
        None => {
            let mut st = c.prepare("SELECT id FROM skills LIMIT 5000")?;
            let it = st.query_map([], |r| r.get::<_, String>(0))?;
            for r in it {
                rows.push((r?, 1.0));
            }
        }
    }

    // Listings (catalogs, installs, categories) and repo stars.
    type Listing = (Vec<String>, Option<i64>, Vec<String>);
    let mut listings: HashMap<String, Listing> = HashMap::new();
    {
        let mut st = c.prepare("SELECT skill_id, catalog, installs, category FROM listings")?;
        let it = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<String>>(3)?))
        })?;
        for r in it {
            let (id, cat, inst, category) = r?;
            let e = listings.entry(id).or_default();
            e.0.push(cat);
            if let Some(i) = inst {
                e.1 = Some(e.1.unwrap_or(0).max(i));
            }
            if let Some(cg) = category
                && !e.2.contains(&cg)
            {
                e.2.push(cg);
            }
        }
    }
    let mut stars: HashMap<String, i64> = HashMap::new();
    {
        let mut st = c.prepare("SELECT source, stars FROM repo_info WHERE stars IS NOT NULL")?;
        for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (s, n) = r?;
            stars.insert(s, n);
        }
    }
    let mut names: HashMap<String, BTreeSet<String>> = HashMap::new();
    {
        let mut st = c.prepare("SELECT name, tree FROM skills WHERE tree IS NOT NULL")?;
        for r in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (n, t) = r?;
            names.entry(n).or_default().insert(t);
        }
    }

    let mut results = Vec::new();
    let mut st = c.prepare(
        "SELECT id, name, description, source, path, tree, commit_sha, ref_name, license, license_class, owner, updated_at,
                has_scripts, allowed_tools, network, compat, kind
         FROM skills WHERE id=?1",
    )?;
    let now_s = now();
    for (id, rel) in rows {
        let Some(r) = st
            .query_row([&id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?.unwrap_or_else(|| "unknown".into()),
                    r.get::<_, String>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, bool>(12)?,
                    r.get::<_, Option<String>>(13)?,
                    r.get::<_, bool>(14)?,
                    r.get::<_, Option<String>>(15)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(16)?.unwrap_or_else(|| "git".into()),
                ))
            })
            .optional()?
        else {
            continue;
        };
        let (
            id,
            name,
            description,
            source,
            path,
            tree,
            commit,
            ref_name,
            license,
            license_class,
            owner,
            updated_at,
            has_scripts,
            allowed_tools,
            network,
            compat,
            kind,
        ) = r;
        let trust = trust_for(&owner, &identity).to_string();
        let (listed_in, installs, categories) = listings.get(&id).cloned().unwrap_or_default();
        let stars_n = stars.get(&source).copied();
        let agents: Vec<String> = compat.split(',').filter(|s| !s.is_empty()).map(String::from).collect();

        // Facet filters.
        if let Some(a) = &f.agent
            && !agents.iter().any(|x| x == a)
        {
            continue;
        }
        if let Some(t) = &f.trust
            && &trust != t
        {
            continue;
        }
        if let Some(l) = &f.license
            && &license_class != l
        {
            continue;
        }
        if let Some(s) = &f.source
            && !source.contains(s.as_str())
            && !listed_in.iter().any(|x| x.contains(s.as_str()))
        {
            continue;
        }
        if let Some(o) = &f.owner
            && !owner.eq_ignore_ascii_case(o)
        {
            continue;
        }
        if let Some(cg) = &f.category
            && !categories.iter().any(|x| x.eq_ignore_ascii_case(cg))
        {
            continue;
        }
        if f.no_scripts && has_scripts {
            continue;
        }
        let is_installed = installed.contains(&id);
        if f.installed && !is_installed {
            continue;
        }

        let mut risk = Vec::new();
        if has_scripts {
            risk.push("scripts".to_string());
        }
        if let Some(t) = &allowed_tools {
            risk.push(if crate::risk::broad_tools(t) { "broad allowed-tools".into() } else { "allowed-tools".into() });
        }
        if network {
            risk.push("network references".into());
        }

        let trust_w = match trust.as_str() {
            "official" => 1.5,
            "yours" | "org" => 1.4,
            _ => 1.0,
        };
        let pop_w = 1.0 + (1.0 + installs.unwrap_or(0) as f64).ln() / 10.0 + (1.0 + stars_n.unwrap_or(0) as f64).ln() / 20.0;
        let fresh_w = match updated_at {
            Some(t) if now_s - t < 180 * 86_400 => 1.0,
            Some(t) if now_s - t < 730 * 86_400 => 0.85,
            Some(_) => 0.7,
            None => 0.9,
        };
        let score = rel.max(0.01) * trust_w * pop_w * fresh_w;
        let variants = names.get(&name).map(|t| t.len().saturating_sub(1)).unwrap_or(0);
        results.push(SearchResult {
            vendored: vendored.contains(&id),
            id,
            name,
            description,
            source,
            path,
            tree,
            commit,
            ref_name,
            license,
            license_class,
            trust,
            installs,
            stars: stars_n,
            listed_in,
            categories,
            updated_at,
            risk,
            agents,
            installed: is_installed,
            duplicates: vec![],
            variants,
            kind,
            score,
        });
    }

    // Group identical copies (same tree) under the best-scoring representative.
    results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    let mut by_tree: BTreeMap<String, usize> = BTreeMap::new();
    let mut grouped: Vec<SearchResult> = Vec::new();
    for r in results {
        if let Some(t) = r.tree.clone() {
            if let Some(&i) = by_tree.get(&t) {
                let rep = &mut grouped[i];
                rep.duplicates.push(r.id.clone());
                for l in r.listed_in {
                    if !rep.listed_in.contains(&l) {
                        rep.listed_in.push(l);
                    }
                }
                rep.installs = match (rep.installs, r.installs) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                };
                continue;
            }
            by_tree.insert(t, grouped.len());
        }
        grouped.push(r);
    }
    grouped.truncate(limit);
    Ok(grouped)
}

pub fn license_class_label(c: &str) -> &str {
    match Class::parse(c) {
        Class::Allow if c == "allow" => "open",
        _ => c,
    }
}

pub fn count(ctx: &Ctx) -> Result<i64> {
    Ok(ctx.state.conn.query_row("SELECT COUNT(*) FROM skills", [], |r| r.get(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts_queries() {
        assert_eq!(fts_query("pdf forms").unwrap(), "\"pdf\" \"forms\"*");
        assert_eq!(fts_query("  "), None);
        assert_eq!(fts_query("a\"b"), Some("\"a\" \"b\"*".into()));
    }

    #[test]
    fn license_classes() {
        assert_eq!(index_license_class(Some("MIT"), false, None), "allow");
        assert_eq!(index_license_class(Some("Proprietary. LICENSE.txt has complete terms"), true, None), "block");
        assert_eq!(index_license_class(Some("Complete terms in LICENSE.txt"), true, None), "unknown");
        assert_eq!(index_license_class(None, false, Some("Apache-2.0")), "allow");
        assert_eq!(index_license_class(None, false, None), "block");
    }
}
