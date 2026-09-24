//! Manifest and lock files (spec §6). Manifests hold intent and are edited with
//! `toml_edit` to preserve user formatting; locks are generated.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use toml_edit::{DocumentMut, Item, Table, value};

pub const WORKSPACE_MANIFEST: &str = "tricks.toml";
pub const WORKSPACE_LOCK: &str = "tricks.lock";
pub const WORK_FILE: &str = "tricks.work.toml";
pub const PUBLISHED_FILE: &str = ".tricks-published";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    #[default]
    Review,
    Auto,
    UnsafeAuto,
    Pinned,
    Paused,
}

impl Policy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Policy::Review => "review",
            Policy::Auto => "auto",
            Policy::UnsafeAuto => "unsafe-auto",
            Policy::Pinned => "pinned",
            Policy::Paused => "paused",
        }
    }
    pub fn parse(s: &str) -> Result<Policy> {
        Ok(match s {
            "review" => Policy::Review,
            "auto" => Policy::Auto,
            "unsafe-auto" => Policy::UnsafeAuto,
            "pinned" => Policy::Pinned,
            "paused" => Policy::Paused,
            _ => bail!("unknown update policy `{s}` (review | auto | unsafe-auto | pinned | paused)"),
        })
    }
    pub fn is_auto(&self) -> bool {
        matches!(self, Policy::Auto | Policy::UnsafeAuto)
    }
}

// ---------------------------------------------------------------- workbench

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub agents: Vec<String>,
    pub fetch_interval: String,
    pub default_sources: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { agents: vec!["claude".into()], fetch_interval: "24h".into(), default_sources: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WbSkill {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<Policy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

impl WbSkill {
    /// The ref request in `@ref` form (`latest` when unspecified).
    pub fn requested_ref(&self) -> String {
        if let Some(b) = &self.branch {
            return format!("refs/heads/{b}");
        }
        if let Some(r) = &self.rev {
            return r.clone();
        }
        self.version.clone().unwrap_or_else(|| "latest".into())
    }
    pub fn policy(&self) -> Policy {
        self.update.unwrap_or_default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkbenchManifest {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub workspaces: BTreeMap<String, String>,
    #[serde(default)]
    pub sources: BTreeMap<String, SourceEntry>,
    #[serde(default)]
    pub skills: BTreeMap<String, WbSkill>,
}

impl WorkbenchManifest {
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockedSkill {
    pub id: String,
    pub name: String,
    /// tag | branch | commit | default-branch
    pub ref_kind: String,
    pub ref_name: String,
    pub commit: String,
    pub tree: String,
    /// Path of the skill at `commit` when it differs from the id's path (after an
    /// upstream rename was followed but not yet updated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkbenchLock {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default, rename = "skill")]
    pub skills: Vec<LockedSkill>,
}

fn one() -> u32 {
    1
}

impl WorkbenchLock {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(WorkbenchLock { version: 1, skills: vec![] });
        }
        let s = std::fs::read_to_string(path)?;
        toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))
    }
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.skills.sort_by(|a, b| a.id.cmp(&b.id));
        let body = toml::to_string_pretty(self)?;
        write_atomic(path, format!("# Generated by New Tricks. Do not edit.\n{body}").as_bytes())
    }
    pub fn get(&self, id: &str) -> Option<&LockedSkill> {
        self.skills.iter().find(|s| s.id == id)
    }
    pub fn upsert(&mut self, s: LockedSkill) {
        self.skills.retain(|x| x.id != s.id);
        self.skills.push(s);
    }
    pub fn remove(&mut self, id: &str) {
        self.skills.retain(|x| x.id != id);
    }
}

// ---------------------------------------------------------------- workspace

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WsSettings {
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
pub struct WsSkill {
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
pub struct WorkspaceManifest {
    #[serde(default)]
    pub workspace: WsSettings,
    #[serde(default)]
    pub skills: BTreeMap<String, WsSkill>,
    #[serde(default)]
    pub lint: LintConfig,
    #[serde(default)]
    pub publish: PublishConfig,
}

impl WorkspaceManifest {
    pub fn load(root: &Path) -> Result<Self> {
        let p = root.join(WORKSPACE_MANIFEST);
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
pub struct WsLocked {
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
pub struct WorkspaceLock {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub skills: BTreeMap<String, WsLocked>,
}

impl WorkspaceLock {
    pub fn load(root: &Path) -> Result<Self> {
        let p = root.join(WORKSPACE_LOCK);
        if !p.exists() {
            return Ok(WorkspaceLock { version: 1, skills: BTreeMap::new() });
        }
        let s = std::fs::read_to_string(&p)?;
        toml::from_str(&s).with_context(|| format!("parsing {}", p.display()))
    }
    pub fn save(&self, root: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self)?;
        write_atomic(&root.join(WORKSPACE_LOCK), format!("# Generated by New Tricks. Do not edit.\n{body}").as_bytes())
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

/// Find the workspace root (directory containing `tricks.toml`) from `start` upward.
pub fn find_workspace(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(d) = cur {
        if d.join(WORKSPACE_MANIFEST).is_file() {
            return Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("24h"), Some(Duration::from_secs(86400)));
        assert_eq!(parse_duration("30m"), Some(Duration::from_secs(1800)));
        assert_eq!(parse_duration("x"), None);
    }

    #[test]
    fn workspace_manifest_roundtrip() {
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
repo    = "../acme-skills-public"
skills  = ["pdf", "deploy-aws"]
exclude = ["evals/**"]

[publish.targets.public.plugins]
documents = ["pdf"]
"#;
        let m: WorkspaceManifest = toml::from_str(s).unwrap();
        assert_eq!(m.skills["pdf"].update, Some(Policy::Review));
        assert_eq!(m.lint.ignore, vec!["NT305"]);
        assert_eq!(m.publish.targets["public"].plugins["documents"], vec!["pdf"]);
    }

    #[test]
    fn inline_item() {
        let s = WbSkill { version: Some("latest".into()), agents: Some(vec!["claude".into()]), ..Default::default() };
        let item = to_inline(&s).unwrap();
        let mut doc = DocumentMut::new();
        table_mut(&mut doc, &["skills"]).insert("github.com/a/b//c", item);
        let out = doc.to_string();
        assert!(out.contains(r#""github.com/a/b//c" = { version = "latest", agents = ["claude"] }"#), "{out}");
    }
}
