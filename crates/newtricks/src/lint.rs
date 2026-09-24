//! `tricks lint` (spec §10): stable rule codes in five families, ruff-style
//! configuration, and conservative `--fix`.

use crate::ctx::Ctx;
use crate::id::valid_skill_name;
use crate::risk::{self, RiskReport};
use crate::skill::SkillDoc;
use crate::workspace::Workspace;
use anyhow::Result;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Finding {
    pub code: String,
    pub severity: String,
    pub skill: String,
    pub file: String,
    pub line: Option<usize>,
    pub message: String,
    pub fixable: bool,
}

#[derive(Debug, Serialize, Default)]
pub struct LintReport {
    pub findings: Vec<Finding>,
    pub errors: usize,
    pub warnings: usize,
    pub fixed: Vec<String>,
}

pub const RULES: &[(&str, &str, &str)] = &[
    ("NT101", "error", "frontmatter `name` is missing"),
    ("NT102", "error", "`name` must be 1-64 lowercase letters, digits and single hyphens"),
    ("NT103", "error", "`name` must match the skill folder name"),
    ("NT104", "error", "`description` is missing or empty"),
    ("NT105", "error", "`description` exceeds 1024 characters"),
    ("NT106", "error", "`compatibility` exceeds 500 characters"),
    ("NT107", "error", "`metadata` must be a map of string keys to string values"),
    ("NT108", "error", "SKILL.md has no valid YAML frontmatter"),
    ("NT201", "error", "broken relative link"),
    ("NT202", "error", "referenced script does not exist"),
    ("NT203", "warning", "absolute or home-relative path"),
    ("NT204", "warning", "reference nested more than one level deep"),
    ("NT205", "warning", "SKILL.md longer than 500 lines"),
    ("NT206", "warning", "SKILL.md body longer than ~5000 tokens"),
    ("NT301", "warning", "description lacks a \"use when …\" trigger clause"),
    ("NT302", "warning", "description shorter than 60 characters"),
    ("NT303", "error", "duplicate skill name in workspace"),
    ("NT304", "warning", "description nearly duplicates another skill's (they will compete for triggering)"),
    ("NT305", "warning", "description written in first or second person"),
    ("NT401", "warning", "agent-specific frontmatter key but that agent is not targeted"),
    ("NT402", "info", "unknown frontmatter key (preserved)"),
    ("NT403", "warning", "non-ASCII characters in frontmatter (APM rejects them)"),
    ("NT501", "error", "hidden or bidirectional Unicode character"),
    ("NT502", "error", "possible secret"),
    ("NT503", "warning", "script downloads and executes remote code"),
    ("NT504", "warning", "overly broad allowed-tools"),
];

fn severity(code: &str) -> &'static str {
    RULES.iter().find(|r| r.0 == code).map(|r| r.1).unwrap_or("warning")
}

const SPEC_KEYS: &[&str] = &["name", "description", "license", "compatibility", "metadata", "allowed-tools"];
const CLAUDE_KEYS: &[&str] =
    &["disable-model-invocation", "user-invocable", "argument-hint", "model", "context", "agent", "hooks", "when_to_use"];

fn link_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"!?\[[^\]]*\]\(\s*<?([^)\s>]+)>?(?:\s+\x22[^\x22]*\x22)?\s*\)").unwrap())
}

fn script_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?:^|[\s`(\x22'])((?:\./)?scripts/[A-Za-z0-9_./-]+\.[A-Za-z0-9]+)").unwrap())
}

fn abs_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?:^|[\s`(\x22'=])(/Users/[^\s`)'"]+|/home/[^\s`)'"]+|~/[^\s`)'"]+|[A-Z]:\\[^\s`)'"]+)"#).unwrap())
}

fn trigger_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)\b(use (it |this )?(when|whenever|for|to|if)|when (the )?user|trigger(s|ed)? (on|when)|whenever|applies when|use for)\b",
        )
        .unwrap()
    })
}

fn person_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^\s*(i |i'm |i can|we |you can|you should|my )").unwrap())
}

pub struct LintContext<'a> {
    pub agents: Vec<String>,
    pub ignore: BTreeSet<String>,
    pub skill: &'a str,
}

fn push(out: &mut Vec<Finding>, lc: &LintContext, code: &str, file: &str, line: Option<usize>, msg: String, fixable: bool) {
    if lc.ignore.contains(code) {
        return;
    }
    out.push(Finding {
        code: code.into(),
        severity: severity(code).into(),
        skill: lc.skill.into(),
        file: file.into(),
        line,
        message: msg,
        fixable,
    });
}

fn fm_line(doc_text: &str, key: &str) -> Option<usize> {
    doc_text.lines().position(|l| l.starts_with(&format!("{key}:"))).map(|i| i + 1)
}

/// Lint one skill directory.
pub fn lint_dir(dir: &Path, lc: &LintContext) -> Vec<Finding> {
    let mut out = Vec::new();
    let folder = dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let skill_md = dir.join("SKILL.md");
    let Ok(text) = std::fs::read_to_string(&skill_md) else {
        push(&mut out, lc, "NT108", "SKILL.md", None, "SKILL.md is missing or unreadable".into(), false);
        return out;
    };
    let doc = SkillDoc::parse(&text);

    // Inline disables via metadata.
    let mut lc_ignore = lc.ignore.clone();
    if let Some(d) = doc.metadata_str("tricks-lint-disable") {
        lc_ignore.extend(d.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).map(String::from));
    }
    let lc = &LintContext { agents: lc.agents.clone(), ignore: lc_ignore, skill: lc.skill };

    // NT1xx spec conformance
    if doc.frontmatter.is_none() || doc.parse_error.is_some() {
        push(
            &mut out,
            lc,
            "NT108",
            "SKILL.md",
            Some(1),
            doc.parse_error
                .clone()
                .map(|e| format!("frontmatter does not parse: {e}"))
                .unwrap_or_else(|| "SKILL.md must start with `---` YAML frontmatter".into()),
            false,
        );
    } else {
        match &doc.name {
            None => push(&mut out, lc, "NT101", "SKILL.md", Some(1), "add `name: <folder-name>`".into(), false),
            Some(n) => {
                if !valid_skill_name(n) {
                    let fixable = valid_skill_name(&n.to_lowercase());
                    push(&mut out, lc, "NT102", "SKILL.md", fm_line(&text, "name"), format!("`{n}` is not a valid name"), fixable);
                }
                if n != &folder && !folder.is_empty() {
                    let fixable = n.to_lowercase() == folder;
                    push(&mut out, lc, "NT103", "SKILL.md", fm_line(&text, "name"), format!("name `{n}` ≠ folder `{folder}`"), fixable);
                }
            }
        }
        match doc.description.as_deref().map(str::trim) {
            None | Some("") => push(
                &mut out,
                lc,
                "NT104",
                "SKILL.md",
                Some(1),
                "add a description saying what the skill does and when to use it".into(),
                false,
            ),
            Some(d) => {
                let n = d.chars().count();
                if n > 1024 {
                    push(&mut out, lc, "NT105", "SKILL.md", fm_line(&text, "description"), format!("{n} characters"), false);
                }
                if !trigger_re().is_match(d) {
                    push(
                        &mut out,
                        lc,
                        "NT301",
                        "SKILL.md",
                        fm_line(&text, "description"),
                        "say when the skill should be used, e.g. \"Use when …\"".into(),
                        false,
                    );
                }
                if n < 60 {
                    push(
                        &mut out,
                        lc,
                        "NT302",
                        "SKILL.md",
                        fm_line(&text, "description"),
                        format!("{n} characters; agents trigger on keywords in the description"),
                        false,
                    );
                }
                if person_re().is_match(d) {
                    push(
                        &mut out,
                        lc,
                        "NT305",
                        "SKILL.md",
                        fm_line(&text, "description"),
                        "write descriptions in the third person (\"Extracts…\", not \"I can…\")".into(),
                        false,
                    );
                }
            }
        }
        if let Some(c) = &doc.compatibility
            && c.chars().count() > 500
        {
            push(&mut out, lc, "NT106", "SKILL.md", fm_line(&text, "compatibility"), format!("{} characters", c.chars().count()), false);
        }
        if let Some(m) = doc.metadata() {
            let ok = match m {
                serde_yaml::Value::Mapping(map) => {
                    map.iter().all(|(k, v)| k.is_string() && (v.is_string() || v.is_number() || v.is_bool()))
                }
                _ => false,
            };
            if !ok {
                push(&mut out, lc, "NT107", "SKILL.md", fm_line(&text, "metadata"), "values must be strings".into(), false);
            }
        }
        // NT4xx agent compatibility
        for k in doc.keys() {
            if CLAUDE_KEYS.contains(&k.as_str()) {
                if !lc.agents.iter().any(|a| a == "claude") {
                    push(
                        &mut out,
                        lc,
                        "NT401",
                        "SKILL.md",
                        fm_line(&text, &k),
                        format!("`{k}` is Claude Code-specific but Claude is not a target agent"),
                        false,
                    );
                }
            } else if !SPEC_KEYS.contains(&k.as_str()) {
                push(&mut out, lc, "NT402", "SKILL.md", fm_line(&text, &k), format!("`{k}` is not in the Agent Skills spec"), false);
            }
        }
        if let Some(fm) = &doc.frontmatter
            && let Some((i, _)) = fm.lines().enumerate().find(|(_, l)| !l.is_ascii())
        {
            push(&mut out, lc, "NT403", "SKILL.md", Some(i + 2), "use ASCII in frontmatter for APM compatibility".into(), false);
        }
    }

    // NT2xx structure
    let lines = text.lines().count();
    if lines > 500 {
        push(&mut out, lc, "NT205", "SKILL.md", None, format!("{lines} lines; move detail into references/"), false);
    }
    let tokens = doc.body.chars().count() / 4;
    if tokens > 5000 {
        push(&mut out, lc, "NT206", "SKILL.md", None, format!("~{tokens} tokens; the whole body loads on activation"), false);
    }
    for e in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| e.file_name() != ".git").flatten() {
        if !e.file_type().is_file() || e.path().extension().map(|x| x != "md").unwrap_or(true) {
            continue;
        }
        let rel = e.path().strip_prefix(dir).unwrap().to_string_lossy().replace('\\', "/");
        let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
        let base = e.path().parent().unwrap();
        let mut in_code = false;
        for (i, line) in t.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            for cap in link_re().captures_iter(line) {
                let target = cap[1].to_string();
                if target.contains("://") || target.starts_with('#') || target.starts_with("mailto:") || target.starts_with('/') {
                    continue;
                }
                let path = target.split(['#', '?']).next().unwrap_or("").replace("%20", " ");
                if path.is_empty() {
                    continue;
                }
                if !base.join(&path).exists() {
                    push(&mut out, lc, "NT201", &rel, Some(i + 1), format!("`{target}` does not exist"), false);
                } else if rel == "SKILL.md"
                    && Path::new(&path).components().filter(|c| matches!(c, std::path::Component::Normal(_))).count() > 2
                {
                    push(&mut out, lc, "NT204", &rel, Some(i + 1), format!("`{path}`: keep references one level deep"), false);
                }
            }
        }
    }
    for (i, line) in doc.body.lines().enumerate() {
        let ln = i + doc.body_line;
        for cap in script_re().captures_iter(line) {
            let p = cap[1].trim_start_matches("./");
            if !dir.join(p).exists() {
                push(&mut out, lc, "NT202", "SKILL.md", Some(ln), format!("`{p}` does not exist"), false);
            }
        }
        for cap in abs_re().captures_iter(line) {
            push(&mut out, lc, "NT203", "SKILL.md", Some(ln), format!("`{}` will not exist on other machines", &cap[1]), false);
        }
    }

    // NT5xx safety
    let r = RiskReport::scan_dir(dir);
    for f in &r.hidden_unicode {
        push(&mut out, lc, "NT501", &f.file, Some(f.line), format!("{} can hide instructions from reviewers", f.detail), false);
    }
    for f in &r.secrets {
        push(&mut out, lc, "NT502", &f.file, Some(f.line), f.detail.clone(), false);
    }
    for f in &r.remote_exec {
        push(&mut out, lc, "NT503", &f.file, Some(f.line), f.detail.clone(), false);
    }
    if r.broad_tools {
        push(
            &mut out,
            lc,
            "NT504",
            "SKILL.md",
            fm_line(&text, "allowed-tools"),
            format!("`{}`: scope tools narrowly, e.g. Bash(git:*)", r.allowed_tools.clone().unwrap_or_default()),
            false,
        );
    }
    let _ = risk::is_hidden_char; // shared scanner
    out
}

fn words(s: &str) -> BTreeSet<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 3).map(String::from).collect()
}

fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let i = a.intersection(b).count() as f64;
    let u = a.union(b).count() as f64;
    if u == 0.0 { 0.0 } else { i / u }
}

pub fn lint_workspace(ctx: &Ctx, ws: &Workspace, names: &[String]) -> Result<LintReport> {
    let agents: Vec<String> = ws.agents(ctx)?.iter().map(|a| a.id.to_string()).collect();
    let mut rep = LintReport::default();
    let mut descs: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, s) in &ws.manifest.skills {
        let dir = ws.root.join(&s.path);
        let doc = std::fs::read_to_string(dir.join("SKILL.md")).map(|t| SkillDoc::parse(&t)).unwrap_or_default();
        if let Some(n) = &doc.name {
            by_name.entry(n.clone()).or_default().push(name.clone());
        }
        if let Some(d) = &doc.description {
            descs.insert(name.clone(), (d.clone(), words(d)));
        }
        if !names.is_empty() && !names.contains(name) {
            continue;
        }
        let mut ignore: BTreeSet<String> = ws.manifest.lint.ignore.iter().cloned().collect();
        if let Some(p) = ws.manifest.lint.per_skill.get(name) {
            ignore.extend(p.ignore.iter().cloned());
        }
        let lc = LintContext { agents: agents.clone(), ignore, skill: name };
        rep.findings.extend(lint_dir(&dir, &lc));
    }
    let ignored = |name: &str, code: &str| {
        ws.manifest.lint.ignore.iter().any(|c| c == code)
            || ws.manifest.lint.per_skill.get(name).map(|p| p.ignore.iter().any(|c| c == code)).unwrap_or(false)
    };
    for (n, skills) in &by_name {
        if skills.len() > 1 {
            for s in skills {
                if (names.is_empty() || names.contains(s)) && !ignored(s, "NT303") {
                    rep.findings.push(Finding {
                        code: "NT303".into(),
                        severity: "error".into(),
                        skill: s.clone(),
                        file: "SKILL.md".into(),
                        line: None,
                        message: format!(
                            "name `{n}` is also used by {}",
                            skills.iter().filter(|x| *x != s).cloned().collect::<Vec<_>>().join(", ")
                        ),
                        fixable: false,
                    });
                }
            }
        }
    }
    let keys: Vec<&String> = descs.keys().collect();
    for i in 0..keys.len() {
        for j in (i + 1)..keys.len() {
            let (a, b) = (keys[i], keys[j]);
            let sim = jaccard(&descs[a].1, &descs[b].1);
            if sim >= 0.7 {
                for (x, y) in [(a, b), (b, a)] {
                    if (names.is_empty() || names.contains(x)) && !ignored(x, "NT304") {
                        rep.findings.push(Finding {
                            code: "NT304".into(),
                            severity: "warning".into(),
                            skill: x.clone(),
                            file: "SKILL.md".into(),
                            line: None,
                            message: format!("{:.0}% similar to `{y}`", sim * 100.0),
                            fixable: false,
                        });
                    }
                }
            }
        }
    }
    rep.findings.sort_by(|a, b| {
        (a.skill.as_str(), a.file.as_str(), a.line, a.code.as_str()).cmp(&(b.skill.as_str(), b.file.as_str(), b.line, b.code.as_str()))
    });
    rep.errors = rep.findings.iter().filter(|f| f.severity == "error").count();
    rep.warnings = rep.findings.iter().filter(|f| f.severity == "warning").count();
    Ok(rep)
}

/// Lint a directory outside a workspace.
pub fn lint_path(dir: &Path) -> LintReport {
    let name = dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    let lc = LintContext {
        agents: vec!["claude".into(), "codex".into(), "copilot".into(), "cursor".into()],
        ignore: BTreeSet::new(),
        skill: &name,
    };
    let findings = lint_dir(dir, &lc);
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    let warnings = findings.iter().filter(|f| f.severity == "warning").count();
    LintReport { findings, errors, warnings, fixed: vec![] }
}

/// Mechanical, unambiguous fixes only: name casing, trailing whitespace, line endings.
pub fn fix_dir(dir: &Path) -> Result<Vec<String>> {
    let mut fixed = Vec::new();
    let folder = dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    for e in walkdir::WalkDir::new(dir).into_iter().filter_entry(|e| e.file_name() != ".git").flatten() {
        if !e.file_type().is_file() {
            continue;
        }
        let ext = e.path().extension().map(|x| x.to_string_lossy().to_string()).unwrap_or_default();
        if !["md", "txt", "yaml", "yml"].contains(&ext.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let rel = e.path().strip_prefix(dir).unwrap().to_string_lossy().to_string();
        let mut t = text.replace("\r\n", "\n");
        t = t.lines().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n");
        if text.ends_with('\n') || text.ends_with("\r\n") {
            t.push('\n');
        }
        if rel == "SKILL.md" {
            let doc = SkillDoc::parse(&t);
            if let Some(n) = doc.name
                && n != n.to_lowercase()
                && n.to_lowercase() == folder
            {
                t = t.replacen(&format!("name: {n}"), &format!("name: {}", n.to_lowercase()), 1);
                fixed.push(format!("{rel}: name `{n}` → `{}`", n.to_lowercase()));
            }
        }
        if t != text {
            if !fixed.iter().any(|f| f.starts_with(&format!("{rel}:"))) {
                fixed.push(format!("{rel}: whitespace/line endings"));
            }
            std::fs::write(e.path(), t)?;
        }
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lc(name: &str) -> LintContext<'_> {
        LintContext { agents: vec!["codex".into()], ignore: BTreeSet::new(), skill: name }
    }

    #[test]
    fn rules_fire() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("my-skill");
        std::fs::create_dir_all(d.join("references")).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            "---\nname: My-Skill\ndescription: Does stuff.\nmodel: opus\nfoo: bar\nallowed-tools: Bash(*)\n---\n# T\n\nSee [ref](references/missing.md) and run scripts/nope.py.\nConfig in /Users/me/x.\nhidden\u{202E}\n",
        )
        .unwrap();
        let f = lint_dir(&d, &lc("my-skill"));
        let codes: BTreeSet<&str> = f.iter().map(|x| x.code.as_str()).collect();
        for c in ["NT102", "NT103", "NT301", "NT302", "NT401", "NT402", "NT201", "NT202", "NT203", "NT501", "NT504"] {
            assert!(codes.contains(c), "missing {c}: {codes:?}");
        }
        let fixed = fix_dir(&d).unwrap();
        assert!(fixed.iter().any(|x| x.contains("my-skill")));
        let f2 = lint_dir(&d, &lc("my-skill"));
        assert!(!f2.iter().any(|x| x.code == "NT103"));
    }

    #[test]
    fn inline_disable_and_clean_skill() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("good");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), "---\nname: good\ndescription: Formats changelogs from commit history. Use when the user asks to write or update a changelog.\nmetadata:\n  tricks-lint-disable: NT203\n---\nSee /Users/x.\n").unwrap();
        let f = lint_dir(&d, &lc("good"));
        assert!(f.is_empty(), "{f:?}");
    }
}
