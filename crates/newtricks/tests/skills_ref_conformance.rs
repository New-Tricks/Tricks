//! NT1xx conformance with the Agent Skills reference validator.
//!
//! Cases ported from `skills-ref` (agentskills/agentskills, Apache-2.0,
//! commit 69ef37e9424c0a7ea9dd2293b559e43ec8176379): `tests/test_validator.py` and
//! `tests/test_parser.py`. Where New Tricks deliberately differs, the case says so.

use newtricks::lint::{Finding, LintContext, lint_dir};
use std::collections::BTreeSet;
use std::path::Path;

fn lint(dir: &Path, strict: bool) -> Vec<Finding> {
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let lc = LintContext { agents: vec!["claude".into()], ignore: BTreeSet::new(), skill: &name, strict };
    lint_dir(dir, &lc)
}

/// Spec-conformance errors only (NT1xx, plus strict-mode NT402).
fn spec_errors(f: &[Finding]) -> Vec<String> {
    f.iter()
        .filter(|x| x.severity == "error" && (x.code.starts_with("NT1") || x.code == "NT402"))
        .map(|x| format!("{} {}", x.code, x.message))
        .collect()
}

fn skill(root: &Path, dir: &str, file: &str, content: &str) -> std::path::PathBuf {
    let d = root.join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(file), content).unwrap();
    d
}

fn case(dir: &str, content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let d = skill(t.path(), dir, "SKILL.md", content);
    (t, d)
}

fn assert_valid(dir: &str, content: &str) {
    let (_t, d) = case(dir, content);
    let e = spec_errors(&lint(&d, false));
    assert!(e.is_empty(), "{dir}: expected valid, got {e:?}");
}

fn assert_code(dir: &str, content: &str, code: &str, needle: &str) {
    let (_t, d) = case(dir, content);
    let e = spec_errors(&lint(&d, false));
    assert!(e.iter().any(|x| x.starts_with(code) && x.contains(needle)), "{dir}: expected {code} containing {needle:?}, got {e:?}");
}

// ---------------------------------------------------------------- test_validator.py

#[test]
fn valid_skill() {
    assert_valid("my-skill", "---\nname: my-skill\ndescription: A test skill\n---\n# My Skill\n");
}

#[test]
fn missing_skill_md() {
    let t = tempfile::tempdir().unwrap();
    let d = t.path().join("my-skill");
    std::fs::create_dir_all(&d).unwrap();
    let e = spec_errors(&lint(&d, false));
    assert!(e.iter().any(|x| x.starts_with("NT108") && x.contains("SKILL.md")), "{e:?}");
}

#[test]
fn invalid_name_uppercase() {
    assert_code("MySkill", "---\nname: MySkill\ndescription: A test skill\n---\nBody\n", "NT102", "lowercase");
}

#[test]
fn name_too_long() {
    let n = "a".repeat(70);
    assert_code(&n, &format!("---\nname: {n}\ndescription: A test skill\n---\nBody\n"), "NT102", "64 character limit");
}

#[test]
fn name_leading_hyphen() {
    assert_code("-my-skill", "---\nname: -my-skill\ndescription: A test skill\n---\nBody\n", "NT102", "start or end with a hyphen");
}

#[test]
fn name_consecutive_hyphens() {
    assert_code("my--skill", "---\nname: my--skill\ndescription: A test skill\n---\nBody\n", "NT102", "consecutive hyphens");
}

#[test]
fn name_invalid_characters() {
    assert_code("my_skill", "---\nname: my_skill\ndescription: A test skill\n---\nBody\n", "NT102", "invalid characters");
}

#[test]
fn name_directory_mismatch() {
    assert_code("wrong-name", "---\nname: correct-name\ndescription: A test skill\n---\nBody\n", "NT103", "folder");
}

/// skills-ref rejects keys outside the spec. New Tricks reports them as info by
/// default (agent-specific keys such as Claude's `model` are legitimate) and as
/// errors with `strict-spec`, which matches the reference exactly.
#[test]
fn unexpected_fields() {
    let content = "---\nname: my-skill\ndescription: A test skill\nunknown_field: should not be here\n---\nBody\n";
    let (_t, d) = case("my-skill", content);
    let default = lint(&d, false);
    assert!(spec_errors(&default).is_empty());
    assert!(default.iter().any(|f| f.code == "NT402" && f.severity == "info"));
    let strict = spec_errors(&lint(&d, true));
    assert!(strict.iter().any(|x| x.starts_with("NT402") && x.contains("unknown_field")), "{strict:?}");
}

#[test]
fn valid_with_all_fields() {
    assert_valid("my-skill", "---\nname: my-skill\ndescription: A test skill\nlicense: MIT\nmetadata:\n  author: Test\n---\nBody\n");
}

#[test]
fn allowed_tools_accepted() {
    assert_valid("my-skill", "---\nname: my-skill\ndescription: A test skill\nallowed-tools: Bash(jq:*) Bash(git:*)\n---\nBody\n");
}

#[test]
fn i18n_chinese_name() {
    assert_valid("技能", "---\nname: 技能\ndescription: A skill with Chinese name\n---\nBody\n");
}

#[test]
fn i18n_russian_name_with_hyphens() {
    assert_valid("мой-навык", "---\nname: мой-навык\ndescription: A skill with Russian name\n---\nBody\n");
}

#[test]
fn i18n_russian_lowercase_valid() {
    assert_valid("навык", "---\nname: навык\ndescription: A skill with Russian lowercase name\n---\nBody\n");
}

#[test]
fn i18n_russian_uppercase_rejected() {
    assert_code("НАВЫК", "---\nname: НАВЫК\ndescription: A skill with Russian uppercase name\n---\nBody\n", "NT102", "lowercase");
}

#[test]
fn description_too_long() {
    let d = "x".repeat(1100);
    assert_code("my-skill", &format!("---\nname: my-skill\ndescription: {d}\n---\nBody\n"), "NT105", "1100");
}

#[test]
fn valid_compatibility() {
    assert_valid("my-skill", "---\nname: my-skill\ndescription: A test skill\ncompatibility: Requires Python 3.11+\n---\nBody\n");
}

#[test]
fn compatibility_too_long() {
    let c = "x".repeat(550);
    assert_code("my-skill", &format!("---\nname: my-skill\ndescription: A test skill\ncompatibility: {c}\n---\nBody\n"), "NT106", "550");
}

#[test]
fn nfkc_normalization() {
    // Directory uses the precomposed form, SKILL.md the decomposed one.
    assert_valid("café", "---\nname: cafe\u{301}\ndescription: A test skill\n---\nBody\n");
}

// ---------------------------------------------------------------- test_parser.py

#[test]
fn missing_frontmatter() {
    assert_code("my-skill", "# No frontmatter here\n", "NT108", "---");
}

#[test]
fn unclosed_frontmatter() {
    assert_code("my-skill", "---\nname: my-skill\ndescription: A test skill\n", "NT108", "---");
}

#[test]
fn invalid_yaml() {
    assert_code("my-skill", "---\nname: [invalid\ndescription: broken\n---\nBody\n", "NT108", "parse");
}

#[test]
fn non_dict_frontmatter() {
    assert_code("my-skill", "---\n- just\n- a list\n---\nBody\n", "NT108", "mapping");
}

#[test]
fn missing_name() {
    assert_code("my-skill", "---\ndescription: A test skill\n---\nBody\n", "NT101", "name");
}

#[test]
fn missing_description() {
    assert_code("my-skill", "---\nname: my-skill\n---\nBody\n", "NT104", "description");
}

/// skills-ref accepts `skill.md`; New Tricks accepts it too but warns (NT110),
/// because Claude Code only loads `SKILL.md`.
#[test]
fn lowercase_skill_md_accepted_with_warning() {
    let t = tempfile::tempdir().unwrap();
    let d = skill(t.path(), "my-skill", "skill.md", "---\nname: my-skill\ndescription: A test skill\n---\nBody\n");
    let f = lint(&d, false);
    assert!(spec_errors(&f).is_empty(), "{f:?}");
    assert!(f.iter().any(|x| x.code == "NT110" && x.severity == "warning"), "{f:?}");
}
