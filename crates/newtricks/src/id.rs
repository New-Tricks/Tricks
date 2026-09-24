//! Skill and source identity (spec §5).
//!
//! Canonical skill ID: `host/owner/repo//path[@ref]`. `//` is mandatory for skills;
//! a reference without `//` denotes a source (repository).

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const DEFAULT_HOST: &str = "github.com";

/// A repository: host + owner path (may contain nested groups on non-GitHub hosts).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceId {
    pub host: String,
    /// `owner/repo`, or `group/subgroup/repo` on hosts with nested groups.
    pub repo_path: String,
}

impl SourceId {
    pub fn new(host: &str, repo_path: &str) -> Self {
        let host = host.to_ascii_lowercase();
        let repo_path = repo_path.trim_matches('/');
        // GitHub owner/repo names are case-insensitive: canonicalize to lowercase so the
        // same repository from different catalogs deduplicates.
        let repo_path =
            if host == "github.com" || host.starts_with("github.") { repo_path.to_ascii_lowercase() } else { repo_path.to_string() };
        SourceId { host, repo_path }
    }

    pub fn owner(&self) -> &str {
        self.repo_path.split('/').next().unwrap_or("")
    }

    pub fn name(&self) -> &str {
        self.repo_path.rsplit('/').next().unwrap_or("")
    }

    /// `owner/repo` as used by the GitHub API.
    pub fn full_name(&self) -> &str {
        &self.repo_path
    }

    pub fn is_github(&self) -> bool {
        self.host == "github.com" || self.host.starts_with("github.")
    }

    /// Parse `[host/]owner/repo` (no `//`).
    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim().trim_end_matches('/');
        if s.contains("//") {
            bail!("`{s}` looks like a skill reference; a source reference has no `//`");
        }
        let segs: Vec<&str> = s.split('/').filter(|x| !x.is_empty()).collect();
        let (host, rest) = split_host(&segs);
        if rest.len() < 2 {
            bail!("invalid source `{s}`: expected [host/]owner/repo");
        }
        if host == DEFAULT_HOST && rest.len() != 2 {
            bail!("invalid GitHub source `{s}`: expected owner/repo");
        }
        let repo_path = rest.join("/").trim_end_matches(".git").to_string();
        Ok(SourceId::new(&host, &repo_path))
    }
}

impl fmt::Display for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.host, self.repo_path)
    }
}

/// Canonical skill identity: source + repository-relative path (`.` for the root).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SkillId {
    pub source: SourceId,
    pub path: String,
}

impl SkillId {
    pub fn new(source: SourceId, path: &str) -> Self {
        let p = path.trim_matches('/');
        SkillId { source, path: if p.is_empty() { ".".to_string() } else { p.to_string() } }
    }

    /// Parse a canonical `host/owner/repo//path` (no ref).
    pub fn parse_canonical(s: &str) -> Result<Self> {
        let spec = SkillSpec::parse(s)?;
        if spec.reference.is_some() {
            bail!("canonical skill id must not contain a ref: `{s}`");
        }
        Ok(SkillId::new(spec.source, &spec.selector))
    }

    /// Folder name of the skill (last path segment, or repo name for root skills).
    pub fn folder_name(&self) -> &str {
        if self.path == "." { self.source.name() } else { self.path.rsplit('/').next().unwrap_or(&self.path) }
    }

    pub fn with_ref(&self, r: &str) -> String {
        format!("{self}@{r}")
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}//{}", self.source, self.path)
    }
}

/// A user-supplied skill reference before resolution: the part after `//` may be a
/// path or a frontmatter name, and the ref is optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSpec {
    pub source: SourceId,
    pub selector: String,
    pub reference: Option<String>,
}

impl SkillSpec {
    /// Parse `[host/]owner/repo//path-or-name[@ref]`, or any supported GitHub URL form.
    pub fn parse(input: &str) -> Result<Self> {
        let input = input.trim();
        if let Some(n) = normalize_url(input)? {
            return match n {
                Normalized::Skill(s) => Ok(s),
                Normalized::Source(src, _) => {
                    bail!("`{input}` refers to repository {src}; add `//<skill>` to reference a skill")
                }
                Normalized::Ambiguous(a) => Ok(a.best_guess()),
            };
        }
        let Some(idx) = input.find("//") else {
            bail!("invalid skill reference `{input}`: expected [host/]owner/repo//skill[@ref] (`//` is required)");
        };
        let (src, rest) = (&input[..idx], &input[idx + 2..]);
        let source = SourceId::parse(src)?;
        let (selector, reference) = match rest.rfind('@') {
            Some(at) => (&rest[..at], Some(rest[at + 1..].to_string())),
            None => (rest, None),
        };
        let selector = selector.trim_matches('/');
        let selector = if selector.is_empty() { "." } else { selector };
        if let Some(r) = &reference
            && r.is_empty()
        {
            bail!("empty ref after `@` in `{input}`");
        }
        Ok(SkillSpec { source, selector: selector.to_string(), reference })
    }

    /// True if the part after `//` must be looked up by name rather than used as a path.
    pub fn looks_like_name(&self) -> bool {
        !self.selector.contains('/') && self.selector != "."
    }
}

impl fmt::Display for SkillSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}//{}", self.source, self.selector)?;
        if let Some(r) = &self.reference {
            write!(f, "@{r}")?;
        }
        Ok(())
    }
}

fn split_host(segs: &[&str]) -> (String, Vec<String>) {
    match segs.first() {
        Some(first) if first.contains('.') || *first == "localhost" || first.contains(':') => {
            (first.to_ascii_lowercase(), segs[1..].iter().map(|s| s.to_string()).collect())
        }
        _ => (DEFAULT_HOST.to_string(), segs.iter().map(|s| s.to_string()).collect()),
    }
}

/// Result of normalizing a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Normalized {
    Source(SourceId, Option<String>),
    Skill(SkillSpec),
    /// `/tree/<a>/<b>/...` where the ref boundary is unknown until refs are listed.
    Ambiguous(AmbiguousTree),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousTree {
    pub source: SourceId,
    /// Segments after `/tree/` or `/blob/`, with a trailing `SKILL.md` removed.
    pub segments: Vec<String>,
}

impl AmbiguousTree {
    /// Resolve using the repository's ref names: the longest prefix that is a ref wins.
    pub fn resolve(&self, refs: &[String]) -> SkillSpec {
        for n in (1..=self.segments.len()).rev() {
            let candidate = self.segments[..n].join("/");
            if refs.iter().any(|r| r == &candidate) {
                return self.spec_at(n);
            }
        }
        self.best_guess()
    }

    /// Without ref information, assume a single-segment ref.
    pub fn best_guess(&self) -> SkillSpec {
        self.spec_at(1)
    }

    fn spec_at(&self, n: usize) -> SkillSpec {
        let r = self.segments[..n].join("/");
        let path = self.segments[n..].join("/");
        SkillSpec { source: self.source.clone(), selector: if path.is_empty() { ".".into() } else { path }, reference: Some(r) }
    }
}

/// Normalize URL forms (https, ssh, scp-style) to canonical references.
/// Returns `Ok(None)` if the input is not a URL.
pub fn normalize_url(input: &str) -> Result<Option<Normalized>> {
    let s = input.trim();
    let (host, path) = if let Some(rest) = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")) {
        let (h, p) = rest.split_once('/').unwrap_or((rest, ""));
        (h.to_string(), p.to_string())
    } else if let Some(rest) = s.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (h, p) = rest.split_once('/').unwrap_or((rest, ""));
        (h.split(':').next().unwrap_or(h).to_string(), p.to_string())
    } else if let Some(rest) = s.strip_prefix("git@") {
        let Some((h, p)) = rest.split_once(':') else { return Ok(None) };
        (h.to_string(), p.to_string())
    } else {
        return Ok(None);
    };
    let host = host.to_ascii_lowercase();
    let path = path.split(['?', '#']).next().unwrap_or("").trim_matches('/').to_string();
    let segs: Vec<&str> = path.split('/').filter(|x| !x.is_empty()).collect();
    if segs.len() < 2 {
        bail!("URL `{input}` does not name a repository");
    }
    let is_github = host == "github.com" || host.starts_with("github.");
    if is_github {
        let source = SourceId::new(&host, &format!("{}/{}", segs[0], segs[1].trim_end_matches(".git")));
        if segs.len() == 2 {
            return Ok(Some(Normalized::Source(source, None)));
        }
        if segs.len() >= 4 && (segs[2] == "tree" || segs[2] == "blob") {
            let mut rest: Vec<String> = segs[3..].iter().map(|s| s.to_string()).collect();
            if rest.last().map(|l| l.eq_ignore_ascii_case("SKILL.md")).unwrap_or(false) {
                rest.pop();
            }
            if rest.len() == 1 {
                return Ok(Some(Normalized::Source(source, Some(rest[0].clone()))));
            }
            return Ok(Some(Normalized::Ambiguous(AmbiguousTree { source, segments: rest })));
        }
        bail!("unsupported GitHub URL form `{input}`");
    }
    // Other hosts: repository is the whole path (nested groups allowed); `/-/tree/` for GitLab.
    if let Some(i) = segs.iter().position(|s| *s == "-") {
        let source = SourceId::new(&host, &segs[..i].join("/"));
        if segs.len() > i + 2 && (segs[i + 1] == "tree" || segs[i + 1] == "blob") {
            let mut rest: Vec<String> = segs[i + 2..].iter().map(|s| s.to_string()).collect();
            if rest.last().map(|l| l.eq_ignore_ascii_case("SKILL.md")).unwrap_or(false) {
                rest.pop();
            }
            return Ok(Some(Normalized::Ambiguous(AmbiguousTree { source, segments: rest })));
        }
        return Ok(Some(Normalized::Source(source, None)));
    }
    let repo_path = segs.join("/");
    Ok(Some(Normalized::Source(SourceId::new(&host, repo_path.trim_end_matches(".git")), None)))
}

/// Parse user input that may be a source reference or a URL to a repository.
pub fn parse_source_input(input: &str) -> Result<SourceId> {
    if let Some(n) = normalize_url(input)? {
        return match n {
            Normalized::Source(s, _) => Ok(s),
            Normalized::Skill(s) => Ok(s.source),
            Normalized::Ambiguous(a) => Ok(a.source),
        };
    }
    SourceId::parse(input)
}

/// Validate a skill name per the Agent Skills spec.
pub fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_form() {
        let s = SkillSpec::parse("anthropics/skills//pdf").unwrap();
        assert_eq!(s.source.to_string(), "github.com/anthropics/skills");
        assert_eq!(s.selector, "pdf");
        assert_eq!(s.reference, None);
        assert!(s.looks_like_name());
    }

    #[test]
    fn parses_expanded_form_with_host_and_ref() {
        let s = SkillSpec::parse("github.mit.edu/ist-org/skills//docx@v1.0.0").unwrap();
        assert_eq!(s.source.host, "github.mit.edu");
        assert_eq!(s.source.repo_path, "ist-org/skills");
        assert_eq!(s.selector, "docx");
        assert_eq!(s.reference.as_deref(), Some("v1.0.0"));
    }

    #[test]
    fn ref_with_slashes_is_after_last_at() {
        let s = SkillSpec::parse("anthropics/skills//skills/pdf@feature/terse").unwrap();
        assert_eq!(s.selector, "skills/pdf");
        assert_eq!(s.reference.as_deref(), Some("feature/terse"));
        assert!(!s.looks_like_name());
    }

    #[test]
    fn requires_double_slash() {
        assert!(SkillSpec::parse("anthropics/skills/pdf").is_err());
    }

    #[test]
    fn root_skill() {
        let s = SkillSpec::parse("owner/my-skill//.").unwrap();
        assert_eq!(s.selector, ".");
        let id = SkillId::new(s.source, &s.selector);
        assert_eq!(id.to_string(), "github.com/owner/my-skill//.");
        assert_eq!(id.folder_name(), "my-skill");
    }

    #[test]
    fn nested_groups_on_other_hosts() {
        let s = SkillSpec::parse("gitlab.com/acme/platform/skills//pdf").unwrap();
        assert_eq!(s.source.repo_path, "acme/platform/skills");
        assert!(SourceId::parse("acme/platform/skills").is_err());
    }

    #[test]
    fn normalizes_urls() {
        let n = normalize_url("https://github.com/anthropics/skills").unwrap().unwrap();
        assert_eq!(n, Normalized::Source(SourceId::new("github.com", "anthropics/skills"), None));

        let n = normalize_url("git@github.com:anthropics/skills.git").unwrap().unwrap();
        assert_eq!(n, Normalized::Source(SourceId::new("github.com", "anthropics/skills"), None));

        let n = normalize_url("https://github.com/anthropics/skills.git").unwrap().unwrap();
        assert_eq!(n, Normalized::Source(SourceId::new("github.com", "anthropics/skills"), None));

        let s = SkillSpec::parse("https://github.com/anthropics/skills/tree/main/skills/pdf").unwrap();
        assert_eq!(s.to_string(), "github.com/anthropics/skills//skills/pdf@main");

        let s = SkillSpec::parse("https://github.com/anthropics/skills/blob/v1.3.0/skills/pdf/SKILL.md").unwrap();
        assert_eq!(s.to_string(), "github.com/anthropics/skills//skills/pdf@v1.3.0");
    }

    #[test]
    fn resolves_slash_branches_by_longest_ref() {
        let Normalized::Ambiguous(a) = normalize_url("https://github.com/o/r/tree/feature/terse/skills/pdf").unwrap().unwrap() else {
            panic!()
        };
        let refs = vec!["main".to_string(), "feature/terse".to_string(), "feature".to_string()];
        let s = a.resolve(&refs);
        assert_eq!(s.reference.as_deref(), Some("feature/terse"));
        assert_eq!(s.selector, "skills/pdf");
    }

    #[test]
    fn skill_names() {
        assert!(valid_skill_name("pdf-processing"));
        assert!(!valid_skill_name("PDF"));
        assert!(!valid_skill_name("-pdf"));
        assert!(!valid_skill_name("pdf--x"));
        assert!(!valid_skill_name(""));
    }
}
