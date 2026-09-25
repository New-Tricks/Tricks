//! Manifest and lock files (spec §6). Manifests hold intent and are edited with
//! `toml_edit` to preserve user formatting; locks are generated.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml_edit::{DocumentMut, Item, Table, value};

pub const REPO_MANIFEST: &str = "tricks.toml";
pub const REPO_LOCK: &str = "tricks.lock";
pub const WORK_FILE: &str = "tricks.work.toml";
pub const PUBLISHED_FILE: &str = ".tricks-published";

/// How a vendored skill follows its upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    /// Upstream changes are reported and merged on request (`tricks update`).
    #[default]
    Review,
    /// Stay on the recorded base; `tricks update` skips it unless named.
    Pinned,
    /// Stop checking upstream.
    Paused,
}

impl Policy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Policy::Review => "review",
            Policy::Pinned => "pinned",
            Policy::Paused => "paused",
        }
    }
}

// ---------------------------------------------------------------- user

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub agents: Vec<String>,
    pub fetch_interval: String,
    /// Live-query catalogs consulted on each search.
    pub live: Vec<String>,
}

pub const LIVE_CATALOGS: &[&str] = &["skills.sh", "tessl", "clawhub", "github"];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            agents: vec!["claude".into()],
            fetch_interval: "24h".into(),
            live: LIVE_CATALOGS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserConfig {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "source-repos")]
    pub source_repos: BTreeMap<String, String>,
    #[serde(default)]
    pub catalogs: BTreeMap<String, CatalogEntry>,
}

/// The user config written on first run: settings with their defaults and the
/// recommended catalogs, all visible and editable.
pub fn initial_user_config() -> String {
    let live = LIVE_CATALOGS.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ");
    let catalogs: String = crate::catalogs::RECOMMENDED.iter().map(|c| format!("\"{c}\" = {{}}\n")).collect();
    format!(
        r#"# New Tricks user config: https://github.com/new-tricks/tricks

[settings]
agents = ["claude"]      # agents to link skills for (a source repo can set its own)
fetch_interval = "24h"   # how often catalogs and upstreams are fetched again
live = [{live}]   # live-query catalogs asked on every search

# Registered by `tricks init`.
[source-repos]

# Where `tricks search` looks. Add with `tricks catalog add`, remove with `tricks catalog remove`,
# and restore the recommended set with `tricks catalog add --recommended`.
[catalogs]
{catalogs}"#
    )
}

impl UserConfig {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let s = std::fs::read_to_string(path)?;
        toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn fetch_interval(&self) -> Duration {
        parse_duration(&self.settings.fetch_interval).unwrap_or(Duration::from_secs(86_400))
    }
}

fn one() -> u32 {
    1
}

// ---------------------------------------------------------------- source repo

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LicenseOverride {
    pub justification: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoSkill {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<Policy>,
    #[serde(default, rename = "use", skip_serializing_if = "Option::is_none")]
    pub use_branch: Option<String>,
    #[serde(default, rename = "license-override", skip_serializing_if = "Option::is_none")]
    pub license_override: Option<LicenseOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PerSkillLint {
    #[serde(default)]
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LintConfig {
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Treat frontmatter keys outside the Agent Skills spec as errors, as the
    /// `skills-ref` reference validator does (default: info/warning).
    #[serde(default, rename = "strict-spec")]
    pub strict_spec: bool,
    #[serde(default, rename = "per-skill")]
    pub per_skill: BTreeMap<String, PerSkillLint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublishTarget {
    pub repo: String,
    #[serde(default = "all_skills")]
    pub skills: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub plugins: BTreeMap<String, Vec<String>>,
    /// Marketplace name; defaults to the target repository directory name.
    #[serde(default)]
    pub marketplace: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

fn all_skills() -> Vec<String> {
    vec!["*".into()]
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PublishConfig {
    #[serde(default)]
    pub targets: BTreeMap<String, PublishTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceRepoManifest {
    #[serde(default, rename = "source-repo")]
    pub source_repo: RepoSettings,
    #[serde(default)]
    pub skills: BTreeMap<String, RepoSkill>,
    #[serde(default)]
    pub lint: LintConfig,
    #[serde(default)]
    pub publish: PublishConfig,
}

impl SourceRepoManifest {
    pub fn load(root: &Path) -> Result<Self> {
        let p = root.join(REPO_MANIFEST);
        let s = std::fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?;
        toml::from_str(&s).with_context(|| format!("parsing {}", p.display()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LicenseRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spdx: Option<String>,
    pub class: String,
    pub source: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepoLocked {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_tree: Option<String>,
    /// Upstream path if it was renamed since vendoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<LicenseRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceRepoLock {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub skills: BTreeMap<String, RepoLocked>,
}

impl SourceRepoLock {
    pub fn load(root: &Path) -> Result<Self> {
        let p = root.join(REPO_LOCK);
        if !p.exists() {
            return Ok(SourceRepoLock { version: 1, skills: BTreeMap::new() });
        }
        let s = std::fs::read_to_string(&p)?;
        toml::from_str(&s).with_context(|| format!("parsing {}", p.display()))
    }
    pub fn save(&self, root: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self)?;
        write_atomic(&root.join(REPO_LOCK), format!("# Generated by New Tricks. Do not edit.\n{body}").as_bytes())
    }
}

/// `tricks.work.toml`: machine-local overrides (gitignored), like `go.work`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkFile {
    #[serde(default, rename = "use")]
    pub use_branch: BTreeMap<String, String>,
}

impl WorkFile {
    pub fn load(root: &Path) -> Result<Self> {
        let p = root.join(WORK_FILE);
        if !p.exists() {
            return Ok(Self::default());
        }
        Ok(toml::from_str(&std::fs::read_to_string(&p)?)?)
    }
}

// ---------------------------------------------------------------- editing helpers

pub fn load_doc(path: &Path) -> Result<DocumentMut> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }
    let s = std::fs::read_to_string(path)?;
    s.parse::<DocumentMut>().with_context(|| format!("parsing {}", path.display()))
}

pub fn save_doc(path: &Path, doc: &DocumentMut) -> Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    write_atomic(path, doc.to_string().as_bytes())
}

/// Get or create a (non-inline) table at `keys`.
pub fn table_mut<'a>(doc: &'a mut DocumentMut, keys: &[&str]) -> &'a mut Table {
    let mut t = doc.as_table_mut();
    for k in keys {
        if !t.contains_key(k) || !t[k].is_table() {
            let mut nt = Table::new();
            nt.set_implicit(keys.len() > 1);
            t.insert(k, Item::Table(nt));
        }
        t = t[k].as_table_mut().unwrap();
    }
    t
}

/// Serialize a serde value into an inline table item.
pub fn to_inline<T: Serialize>(v: &T) -> Result<Item> {
    let s = toml::to_string(&Wrapper { v })?;
    let doc: DocumentMut = s.parse()?;
    let item = doc.get("v").cloned().unwrap_or(Item::None);
    Ok(match item {
        Item::Table(t) => Item::Value(toml_edit::Value::InlineTable(t.into_inline_table())),
        other => other,
    })
}

#[derive(Serialize)]
struct Wrapper<'a, T: Serialize> {
    v: &'a T,
}

pub fn str_value(s: &str) -> Item {
    value(s)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.tmp{}", path.file_name().unwrap().to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = num.parse().ok()?;
    let secs = match unit.trim() {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86_400,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

/// Find the source repo root (directory containing `tricks.toml`) from `start` upward.
/// Where branch experiments are checked out, inside the source repo (git-ignored).
pub const WORK_DIR: &str = ".tricks/work";

/// The source repo containing `start`. Inside an experiment worktree
/// (`<repo>/.tricks/work/<branch>/…`, itself a checkout with a `tricks.toml`) this is
/// the repo the worktree belongs to.
pub fn find_source_repo(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(d) = cur {
        if d.join(REPO_MANIFEST).is_file() {
            if let Some(work) = d.parent()
                && work.ends_with(WORK_DIR)
                && let Some(repo) = work.parent().and_then(Path::parent)
                && repo.join(REPO_MANIFEST).is_file()
            {
                return Some(repo.to_path_buf());
            }
            return Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Keys of a serialized value, as the TOML file spells them.
    fn keys<T: Serialize>(v: &T) -> BTreeSet<String> {
        serde_json::to_value(v).unwrap().as_object().unwrap().keys().cloned().collect()
    }

    fn schema_keys(v: &serde_json::Value) -> BTreeSet<String> {
        v["properties"].as_object().unwrap().keys().cloned().collect()
    }

    /// The extension's `tricks.toml` schema (completion and validation) lists exactly the
    /// keys the code reads.
    #[test]
    fn json_schema_matches_config() {
        let schema: serde_json::Value = serde_json::from_str(include_str!("../../../extension/schemas/tricks.schema.json")).unwrap();
        let p = &schema["properties"];
        let skill = RepoSkill {
            path: "p".into(),
            upstream: Some("u".into()),
            track: Some("t".into()),
            update: Some(Policy::Review),
            use_branch: Some("b".into()),
            license_override: Some(LicenseOverride { justification: "j".into() }),
        };
        let target =
            PublishTarget { marketplace: Some("m".into()), owner: Some("o".into()), description: Some("d".into()), ..Default::default() };
        let repo = RepoSettings { name: Some("n".into()), agents: vec!["claude".into()] };
        let catalog = CatalogEntry { kind: Some("repo".into()) };
        let mut top = keys(&UserConfig::default());
        top.extend(keys(&SourceRepoManifest::default()));
        assert_eq!(schema_keys(&schema), top, "top-level tables");
        assert_eq!(schema_keys(&p["settings"]), keys(&Settings::default()), "[settings]");
        assert_eq!(schema_keys(&p["catalogs"]["additionalProperties"]), keys(&catalog), "[catalogs.*]");
        assert_eq!(schema_keys(&p["source-repo"]), keys(&repo), "[source-repo]");
        assert_eq!(schema_keys(&p["skills"]["additionalProperties"]), keys(&skill), "[skills.*]");
        assert_eq!(schema_keys(&p["lint"]), keys(&LintConfig::default()), "[lint]");
        assert_eq!(schema_keys(&p["publish"]["properties"]["targets"]["additionalProperties"]), keys(&target), "[publish.targets.*]");
        let policies: Vec<&str> = schema["definitions"]["policy"]["enum"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(policies, [Policy::Review, Policy::Pinned, Policy::Paused].map(|p| p.as_str()));
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("24h"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_duration("x"), None);
    }

    #[test]
    fn source_repo_manifest_roundtrip() {
        let s = r#"
[skills.pdf]
path     = "skills/pdf"
upstream = "github.com/anthropics/skills//skills/pdf"
track    = "main"
update   = "review"

[skills.deploy-aws]
path = "skills/deploy-aws"

[lint]
ignore = ["NT305"]

[publish.targets.public]
repo    = "acme/acme-skills-public"
skills  = ["pdf", "deploy-aws"]
exclude = ["evals/**"]

[publish.targets.public.plugins]
documents = ["pdf"]
"#;
        let m: SourceRepoManifest = toml::from_str(s).unwrap();
        assert_eq!(m.skills["pdf"].update, Some(Policy::Review));
        assert_eq!(m.lint.ignore, vec!["NT305"]);
        assert_eq!(m.publish.targets["public"].plugins["documents"], vec!["pdf"]);
    }

    #[test]
    fn inline_item() {
        let o = LicenseOverride { justification: "separate agreement".into() };
        let item = to_inline(&o).unwrap();
        let mut doc = DocumentMut::new();
        table_mut(&mut doc, &["skills", "pdf"]).insert("license-override", item);
        let out = doc.to_string();
        assert!(out.contains(r#"license-override = { justification = "separate agreement" }"#), "{out}");
    }
}
