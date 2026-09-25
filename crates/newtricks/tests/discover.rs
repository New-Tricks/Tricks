//! Discovery: `info` and `view`, and the catalogs written into a new user config.

mod common;
use common::*;

#[test]
fn info_shows_metadata_and_view_shows_content() {
    let s = Sandbox::new();
    s.upstream(
        "acme",
        "skills",
        &[
            ("skills/hello/SKILL.md", &skill_md("hello", "Say hello. Use when greeting.", "# Hello\n\nBe **kind**.\n")),
            ("skills/hello/references/tone.md", "# Tone\n\nWarm.\n"),
        ],
    );
    let info = s.json(&["info", "acme/skills//hello"]);
    assert_eq!(info["name"], "hello");
    assert!(info["frontmatter"].as_str().unwrap().contains("description: Say hello."), "{info}");
    assert!(info.get("body").is_none(), "info carries no body: {info}");
    assert!(info["files"].to_string().contains("references/tone.md"));
    let human = s.ok(&["info", "acme/skills//hello"]);
    assert!(human.contains("frontmatter:") && human.contains("tricks view"), "{human}");
    assert!(!human.contains("Be **kind**"), "{human}");

    // Not a terminal: the file is printed as is (agents and pipes get plain Markdown).
    // (Line endings normalized: git may check files out with CRLF on Windows.)
    let lf = |t: String| t.replace("\r\n", "\n");
    let raw = lf(s.ok(&["view", "acme/skills//hello"]));
    assert!(raw.starts_with("---\nname: hello") && raw.contains("Be **kind**."), "{raw}");
    let tone = lf(s.ok(&["view", "acme/skills//hello", "references/tone.md"]));
    assert_eq!(tone, "# Tone\n\nWarm.\n");
    let j = s.json(&["view", "acme/skills//hello", "references/tone.md"]);
    assert_eq!(j["path"], "references/tone.md");
    assert_eq!(lf(j["content"].as_str().unwrap().to_string()), "# Tone\n\nWarm.\n");
}

#[test]
fn a_new_user_config_lists_the_recommended_catalogs() {
    let s = Sandbox::new();
    let cfg = s.config.join("tricks.toml");
    std::fs::remove_file(&cfg).unwrap();
    let l = s.json(&["--offline", "catalog", "list"]);
    let keys: Vec<&str> = l.as_array().unwrap().iter().map(|c| c["key"].as_str().unwrap()).collect();
    assert!(keys.contains(&"github.com/anthropics/skills") && keys.len() >= 5, "{l}");
    let text = read(&cfg);
    assert!(text.contains("[catalogs]") && text.contains("\"github.com/anthropics/skills\" = {}") && text.contains("[settings]"), "{text}");
    // Removing one really removes it; --recommended brings it back.
    s.ok(&["--offline", "catalog", "remove", "github.com/obra/superpowers"]);
    assert!(!read(&cfg).contains("obra/superpowers"));
    let r = s.json(&["--offline", "catalog", "add", "--recommended"]);
    assert_eq!(r["added"], serde_json::json!(["github.com/obra/superpowers"]), "{r}");
    assert!(read(&cfg).contains("obra/superpowers"));
    // A config that exists is never rewritten with defaults.
    let s2 = Sandbox::new();
    s2.ok(&["--offline", "catalog", "list"]);
    assert!(!read(&s2.config.join("tricks.toml")).contains("anthropics"));
}
