//! Licence detection and policy (spec §11 licence policy).

use crate::config::LicenseRecord;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    Allow,
    WeakCopyleft,
    StrongCopyleft,
    NonCommercial,
    Block,
}

impl Class {
    pub fn as_str(&self) -> &'static str {
        match self {
            Class::Allow => "allow",
            Class::WeakCopyleft => "weak-copyleft",
            Class::StrongCopyleft => "strong-copyleft",
            Class::NonCommercial => "non-commercial",
            Class::Block => "block",
        }
    }
    pub fn parse(s: &str) -> Class {
        match s {
            "allow" => Class::Allow,
            "weak-copyleft" => Class::WeakCopyleft,
            "strong-copyleft" => Class::StrongCopyleft,
            "non-commercial" => Class::NonCommercial,
            _ => Class::Block,
        }
    }
    const ORDER: [Class; 5] = [Class::Allow, Class::WeakCopyleft, Class::StrongCopyleft, Class::NonCommercial, Class::Block];
}

const ALLOW: &[&str] = &[
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "0BSD",
    "ISC",
    "Zlib",
    "Unlicense",
    "CC0-1.0",
    "CC-BY-4.0",
    "CC-BY-3.0",
    "BSL-1.0",
    "Python-2.0",
    "PSF-2.0",
    "WTFPL",
    "X11",
    "BlueOak-1.0.0",
    "UPL-1.0",
    "NCSA",
    "PostgreSQL",
    "Artistic-2.0",
];

pub fn classify_id(id: &str) -> Class {
    let id = id.trim().trim_end_matches('+');
    if ALLOW.iter().any(|a| a.eq_ignore_ascii_case(id)) {
        return Class::Allow;
    }
    let u = id.to_ascii_uppercase();
    if u.starts_with("MPL-") || u.starts_with("EPL-") || u.starts_with("LGPL-") || u.starts_with("CDDL-") || u == "MS-RL" {
        return Class::WeakCopyleft;
    }
    if u.starts_with("GPL-") || u.starts_with("AGPL-") || u.starts_with("CC-BY-SA-") || u.starts_with("EUPL-") || u.starts_with("OSL-") {
        return Class::StrongCopyleft;
    }
    if u.starts_with("CC-BY-NC") || u.starts_with("POLYFORM-NONCOMMERCIAL") {
        return Class::NonCommercial;
    }
    Class::Block
}

/// Classify an SPDX expression: `OR` takes the most permissive branch, `AND` the most
/// restrictive. Returns None if it does not parse.
pub fn classify_expression(expr: &str) -> Option<Class> {
    let parsed = spdx::Expression::parse_mode(expr, spdx::ParseMode::LAX).ok()?;
    for threshold in Class::ORDER {
        let ok = parsed.evaluate(|req| {
            let id = match &req.license {
                spdx::LicenseItem::Spdx { id, .. } => id.name.to_string(),
                spdx::LicenseItem::Other(o) => o.to_string(),
            };
            classify_id(&id) <= threshold
        });
        if ok {
            return Some(threshold);
        }
    }
    Some(Class::Block)
}

fn store() -> &'static spdx::detection::Store {
    static S: OnceLock<spdx::detection::Store> = OnceLock::new();
    S.get_or_init(|| spdx::detection::Store::load_inline().expect("embedded licence store"))
}

const RESTRICTIVE: &[&str] = &[
    "all rights reserved",
    "may not",
    "prohibited",
    "proprietary",
    "not permitted",
    "terms of service",
    "developer terms",
    "without the prior written",
    "no license",
    "confidential",
];

#[derive(Debug, Clone)]
pub struct TextDetection {
    pub spdx: Option<String>,
    pub confidence: f32,
    pub restrictive: bool,
}

pub fn detect_text(text: &str) -> TextDetection {
    let m = store().analyze(&spdx::detection::TextData::new(text));
    let lower = text.to_ascii_lowercase();
    let restrictive = RESTRICTIVE.iter().any(|k| lower.contains(k));
    if m.score >= 0.9 {
        TextDetection { spdx: Some(m.name.to_string()), confidence: m.score, restrictive: false }
    } else {
        TextDetection { spdx: None, confidence: m.score, restrictive }
    }
}

/// Inputs gathered from a skill directory and its repository.
#[derive(Debug, Default)]
pub struct LicenseInputs {
    /// Contents of licence files found directly in the skill directory.
    pub skill_files: Vec<(String, String)>,
    pub frontmatter: Option<String>,
    pub repo_root: Option<String>,
    /// SPDX id from the GitHub API (repository root), if queried.
    pub api_spdx: Option<String>,
}

pub fn is_license_file(name: &str) -> bool {
    let u = name.to_ascii_uppercase();
    u.starts_with("LICENSE") || u.starts_with("LICENCE") || u.starts_with("COPYING")
}

pub fn is_notice_file(name: &str) -> bool {
    name.to_ascii_uppercase().starts_with("NOTICE")
}

/// Detection order: skill-dir file → frontmatter → repository root → API. The most
/// restrictive result among the sources that yield an answer wins.
pub fn detect(inputs: &LicenseInputs) -> LicenseRecord {
    let mut results: Vec<(Class, Option<String>, &'static str, f32)> = Vec::new();
    let mut defer_to_file = false;

    for (_name, text) in &inputs.skill_files {
        let d = detect_text(text);
        match d.spdx {
            Some(id) => results.push((classify_id(&id), Some(id), "skill-file", d.confidence)),
            None => {
                results.push((Class::Block, Some(if d.restrictive { "Proprietary" } else { "Unknown" }.into()), "skill-file", d.confidence))
            }
        }
    }
    if let Some(fm) = inputs.frontmatter.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let lower = fm.to_ascii_lowercase();
        if lower.contains("proprietary") || lower.contains("all rights reserved") {
            results.push((Class::Block, Some("Proprietary".into()), "frontmatter", 1.0));
        } else if let Some(c) = classify_expression(fm) {
            results.push((c, Some(fm.to_string()), "frontmatter", 1.0));
        } else if lower.contains("license") || lower.contains("terms") {
            defer_to_file = true; // e.g. "Complete terms in LICENSE.txt"
        } else {
            results.push((Class::Block, Some(fm.to_string()), "frontmatter", 0.5));
        }
    }
    let have_skill_level = !results.is_empty();
    if !have_skill_level {
        if let Some(text) = &inputs.repo_root {
            let d = detect_text(text);
            match d.spdx {
                Some(id) => results.push((classify_id(&id), Some(id), "repo-root", d.confidence)),
                None => results.push((
                    Class::Block,
                    Some(if d.restrictive { "Proprietary" } else { "Unknown" }.into()),
                    "repo-root",
                    d.confidence,
                )),
            }
        } else if let Some(id) = inputs.api_spdx.as_deref().filter(|s| *s != "NOASSERTION") {
            results.push((classify_id(id), Some(id.to_string()), "github-api", 0.8));
        }
    }
    if results.is_empty() {
        let note = if defer_to_file { "frontmatter refers to a licence file that is missing" } else { "no licence found" };
        return LicenseRecord { spdx: None, class: Class::Block.as_str().into(), source: format!("none ({note})"), confidence: 0.0 };
    }
    results.sort_by(|a, b| b.0.cmp(&a.0));
    let (class, spdx, source, confidence) = results.into_iter().next().unwrap();
    LicenseRecord { spdx, class: class.as_str().into(), source: source.into(), confidence }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Allow,
    Warn(String),
    Block(String),
}

/// Publish gate for one skill (spec §11 policy table).
pub fn gate(record: &LicenseRecord, public: bool, accept_copyleft: bool, overridden: bool) -> Gate {
    let class = Class::parse(&record.class);
    let what = record.spdx.clone().unwrap_or_else(|| "no licence".into());
    if overridden && class != Class::Allow {
        return Gate::Warn(format!("{what} ({}) allowed by override", class.as_str()));
    }
    match (class, public) {
        (Class::Allow, _) => Gate::Allow,
        (Class::WeakCopyleft, true) => Gate::Warn(format!("{what} is weak copyleft: modifications stay under it")),
        (Class::WeakCopyleft, false) => Gate::Allow,
        (Class::StrongCopyleft, true) if accept_copyleft => Gate::Warn(format!("{what} is strong copyleft (accepted)")),
        (Class::StrongCopyleft, true) => Gate::Block(format!("{what} is strong copyleft; pass --accept-copyleft to publish")),
        (Class::StrongCopyleft, false) => Gate::Warn(format!("{what} is strong copyleft")),
        (Class::NonCommercial, true) => Gate::Block(format!("{what} is non-commercial")),
        (Class::Block, true) => Gate::Block(format!("{what} does not permit redistribution ({})", record.source)),
        (Class::NonCommercial | Class::Block, false) => Gate::Warn(format!("{what}: terms may forbid redistribution even privately")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIT: &str = "MIT License\n\nCopyright (c) 2024 Acme\n\nPermission is hereby granted, free of charge, to any person obtaining a copy\nof this software and associated documentation files (the \"Software\"), to deal\nin the Software without restriction, including without limitation the rights\nto use, copy, modify, merge, publish, distribute, sublicense, and/or sell\ncopies of the Software, and to permit persons to whom the Software is\nfurnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all\ncopies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR\nIMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,\nFITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE\nAUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER\nLIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,\nOUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE\nSOFTWARE.\n";

    #[test]
    fn classifies_ids_and_expressions() {
        assert_eq!(classify_id("MIT"), Class::Allow);
        assert_eq!(classify_id("GPL-3.0-or-later"), Class::StrongCopyleft);
        assert_eq!(classify_id("LGPL-2.1-only"), Class::WeakCopyleft);
        assert_eq!(classify_id("CC-BY-NC-4.0"), Class::NonCommercial);
        assert_eq!(classify_id("BUSL-1.1"), Class::Block);
        assert_eq!(classify_expression("MIT OR GPL-3.0-only"), Some(Class::Allow));
        assert_eq!(classify_expression("MIT AND GPL-3.0-only"), Some(Class::StrongCopyleft));
    }

    #[test]
    fn detects_mit_text() {
        let d = detect_text(MIT);
        assert_eq!(d.spdx.as_deref(), Some("MIT"));
    }

    #[test]
    fn proprietary_wins() {
        let r = detect(&LicenseInputs {
            skill_files: vec![("LICENSE.txt".into(), "© 2025 Anthropic, PBC. All rights reserved. You may not distribute.".into())],
            frontmatter: Some("Proprietary. LICENSE.txt has complete terms".into()),
            ..Default::default()
        });
        assert_eq!(r.class, "block");
        let r = detect(&LicenseInputs { frontmatter: Some("Complete terms in LICENSE.txt".into()), ..Default::default() });
        assert_eq!(r.class, "block");
        let r = detect(&LicenseInputs { skill_files: vec![("LICENSE".into(), MIT.into())], ..Default::default() });
        assert_eq!(r.class, "allow");
        assert_eq!(gate(&r, true, false, false), Gate::Allow);
    }
}
