//! `SKILL.md` parsing. Missing or invalid metadata never prevents reading a file.

use serde::Serialize;
use serde_yaml::{Mapping, Value};

#[derive(Debug, Clone, Default, Serialize)]
pub struct SkillDoc {
    /// Raw YAML between the `---` fences, if present.
    pub frontmatter: Option<String>,
    #[serde(skip)]
    pub meta: Option<Mapping>,
    pub parse_error: Option<String>,
    pub body: String,
    /// 1-based line on which the body starts.
    pub body_line: usize,
    pub name: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<String>,
}

impl SkillDoc {
    pub fn parse(text: &str) -> SkillDoc {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut doc = SkillDoc { body: text.to_string(), body_line: 1, ..Default::default() };
        let Some((fm, body, body_line)) = split_frontmatter(text) else {
            return doc;
        };
        doc.frontmatter = Some(fm.to_string());
        doc.body = body.to_string();
        doc.body_line = body_line;
        match serde_yaml::from_str::<Value>(fm) {
            Ok(Value::Mapping(m)) => {
                doc.name = str_field(&m, "name");
                doc.description = str_field(&m, "description");
                doc.license = str_field(&m, "license");
                doc.compatibility = str_field(&m, "compatibility");
                doc.allowed_tools = match m.get("allowed-tools") {
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(Value::Sequence(seq)) => Some(seq.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" ")),
                    _ => None,
                };
                doc.meta = Some(m);
            }
            Ok(Value::Null) => doc.meta = Some(Mapping::new()),
            Ok(_) => doc.parse_error = Some("frontmatter is not a mapping".into()),
            Err(e) => doc.parse_error = Some(e.to_string()),
        }
        doc
    }

    pub fn keys(&self) -> Vec<String> {
        self.meta.as_ref().map(|m| m.keys().filter_map(|k| k.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    pub fn metadata(&self) -> Option<&Value> {
        self.meta.as_ref()?.get("metadata")
    }

    pub fn metadata_str(&self, key: &str) -> Option<String> {
        match self.metadata()? {
            Value::Mapping(m) => m.get(key).and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                Value::Bool(b) => Some(b.to_string()),
                _ => None,
            }),
            _ => None,
        }
    }
}

fn str_field(m: &Mapping, key: &str) -> Option<String> {
    match m.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Returns (frontmatter, body, body_start_line).
fn split_frontmatter(text: &str) -> Option<(&str, &str, usize)> {
    let first_nl = text.find('\n')?;
    if text[..first_nl].trim_end() != "---" {
        return None;
    }
    let rest = &text[first_nl + 1..];
    let mut offset = 0;
    for (line_no, line) in (2..).zip(rest.split_inclusive('\n')) {
        let t = line.trim_end();
        if t == "---" || t == "..." {
            let fm = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return Some((fm, body, line_no + 1));
        }
        offset += line.len();
    }
    None
}

/// Set `metadata.<key>` in a SKILL.md's frontmatter by text editing (keeps formatting
/// and comments of everything else). Adds a frontmatter block if missing.
pub fn set_metadata_value(text: &str, key: &str, value: &str) -> String {
    let quoted = format!("\"{}\"", value.replace('"', "\\\""));
    let Some((fm, body, _)) = split_frontmatter(text) else {
        return format!("---\nmetadata:\n  {key}: {quoted}\n---\n{text}");
    };
    let mut lines: Vec<String> = fm.lines().map(String::from).collect();
    let meta_idx = lines.iter().position(|l| l.trim_end() == "metadata:");
    match meta_idx {
        Some(i) => {
            let mut j = i + 1;
            let mut replaced = false;
            while j < lines.len() && (lines[j].starts_with(' ') || lines[j].starts_with('\t') || lines[j].trim().is_empty()) {
                let t = lines[j].trim_start();
                if t.starts_with(&format!("{key}:")) {
                    let indent: String = lines[j].chars().take_while(|c| c.is_whitespace()).collect();
                    lines[j] = format!("{indent}{key}: {quoted}");
                    replaced = true;
                    break;
                }
                j += 1;
            }
            if !replaced {
                lines.insert(i + 1, format!("  {key}: {quoted}"));
            }
        }
        None => {
            lines.push("metadata:".to_string());
            lines.push(format!("  {key}: {quoted}"));
        }
    }
    format!("---\n{}\n---\n{}", lines.join("\n"), body)
}

/// Remove `metadata.<prefix>*` keys (e.g. `tricks-`), dropping an emptied block.
pub fn strip_metadata_prefix(text: &str, prefix: &str) -> String {
    let Some((fm, body, _)) = split_frontmatter(text) else { return text.to_string() };
    let mut out: Vec<String> = Vec::new();
    let mut in_meta = false;
    for line in fm.lines() {
        if line.trim_end() == "metadata:" {
            in_meta = true;
            out.push(line.to_string());
            continue;
        }
        if in_meta {
            if line.starts_with(' ') || line.starts_with('\t') {
                if line.trim_start().starts_with(prefix) {
                    continue;
                }
            } else {
                in_meta = false;
            }
        }
        out.push(line.to_string());
    }
    // Drop `metadata:` if nothing indented follows it.
    let mut cleaned: Vec<String> = Vec::new();
    for (i, l) in out.iter().enumerate() {
        if l.trim_end() == "metadata:" {
            let next_indented = out.get(i + 1).map(|n| n.starts_with(' ') || n.starts_with('\t')).unwrap_or(false);
            if !next_indented {
                continue;
            }
        }
        cleaned.push(l.clone());
    }
    format!("---\n{}\n---\n{}", cleaned.join("\n"), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "---\nname: pdf\ndescription: Handle PDFs. Use when working with PDF files.\nlicense: MIT\nmetadata:\n  author: acme\n  tricks-lint-disable: NT203\n---\n# PDF\n\nBody.\n";

    #[test]
    fn parses() {
        let d = SkillDoc::parse(S);
        assert_eq!(d.name.as_deref(), Some("pdf"));
        assert_eq!(d.license.as_deref(), Some("MIT"));
        assert_eq!(d.metadata_str("author").as_deref(), Some("acme"));
        assert!(d.body.starts_with("# PDF"));
        assert_eq!(d.body_line, 9);
    }

    #[test]
    fn no_frontmatter_is_readable() {
        let d = SkillDoc::parse("# Hello\n");
        assert!(d.frontmatter.is_none());
        assert_eq!(d.body, "# Hello\n");
    }

    #[test]
    fn sets_and_strips_metadata() {
        let t = set_metadata_value(S, "version", "1.3.0");
        let d = SkillDoc::parse(&t);
        assert_eq!(d.metadata_str("version").as_deref(), Some("1.3.0"));
        assert_eq!(d.metadata_str("author").as_deref(), Some("acme"));
        let t2 = strip_metadata_prefix(&t, "tricks-");
        let d2 = SkillDoc::parse(&t2);
        assert!(d2.metadata_str("tricks-lint-disable").is_none());
        assert_eq!(d2.metadata_str("version").as_deref(), Some("1.3.0"));

        let only = "---\nname: x\nmetadata:\n  tricks-lint-disable: SB1\n---\nbody\n";
        let s = strip_metadata_prefix(only, "tricks-");
        assert!(!s.contains("metadata:"));
        let added = set_metadata_value("---\nname: x\n---\nb\n", "version", "2.0.0");
        assert_eq!(SkillDoc::parse(&added).metadata_str("version").as_deref(), Some("2.0.0"));
    }
}
