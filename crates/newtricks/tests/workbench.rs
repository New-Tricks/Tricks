//! M2 acceptance: workbench installs, policies, locks, rollback, links.
// Asserts on symlinked placements; Windows deploys copies (spec §8), so these run on Unix.
#![cfg(unix)]

mod common;
use common::*;

fn hello_repo(s: &Sandbox) -> std::path::PathBuf {
    let r = s.upstream(
        "acme",
        "skills",
        &[
            ("skills/hello/SKILL.md", &skill_md("hello", "Say hello. Use when greeting.", "# Hello\n\nv1\n")),
            ("skills/other/SKILL.md", &skill_md("other", "Another skill. Use when testing.", "Other\n")),
        ],
    );
    git(&r, &["tag", "v1.0.0"]);
    r
}

#[test]
fn short_id_and_url_resolve_to_same_canonical() {
    let s = Sandbox::new();
    let r = hello_repo(&s);
    let a = s.json(&["show", "acme/skills//hello"]);
    let b = s.json(&["show", "https://github.com/acme/skills/tree/v1.0.0/skills/hello"]);
    let c = s.json(&["show", "acme/skills//skills/hello@v1.0.0"]);
    assert_eq!(a["canonical"], "github.com/acme/skills//skills/hello@v1.0.0");
    assert_eq!(a["canonical"], b["canonical"]);
    assert_eq!(a["commit"], b["commit"]);
    assert_eq!(a["commit"], c["commit"]);
    assert_eq!(a["license"]["class"], "allow");
    // Branch with slashes in a /tree/ URL resolves by longest matching ref.
    git(&r, &["checkout", "-qb", "feature/terse"]);
    write(&r.join("skills/hello/SKILL.md"), &skill_md("hello", "Say hello. Use when greeting.", "terse\n"));
    let fc = commit_all(&r, "terse");
    let d = s.json(&["show", "https://github.com/acme/skills/tree/feature/terse/skills/hello"]);
    assert_eq!(d["commit"], fc);
    assert_eq!(d["ref_name"], "feature/terse");
}

#[test]
fn add_places_links_and_writes_manifest_and_lock() {
    let s = Sandbox::new();
    hello_repo(&s);
    let r = s.json(&["add", "acme/skills//hello"]);
    assert_eq!(r["canonical"], "github.com/acme/skills//skills/hello@v1.0.0");
    let claude = s.home.join(".claude/skills/hello");
    let codex = s.home.join(".agents/skills/hello");
    assert!(std::fs::symlink_metadata(&claude).unwrap().file_type().is_symlink());
    assert!(codex.join("SKILL.md").exists());
    // Store entries are read-only.
    let target = std::fs::read_link(&claude).unwrap();
    assert!(std::fs::metadata(target.join("SKILL.md")).unwrap().permissions().readonly());
    let manifest = std::fs::read_to_string(s.config.join("tricks.toml")).unwrap();
    assert!(manifest.contains(r#""github.com/acme/skills//skills/hello" = { version = "latest" }"#), "{manifest}");
    let lock = std::fs::read_to_string(s.config.join("tricks.lock")).unwrap();
    assert!(lock.contains("ref_name = \"v1.0.0\""));
    // Copilot always gets a copy (microsoft/vscode#315979).
    s.ok(&["add", "acme/skills//other", "--agents", "copilot"]);
    let cp = s.home.join(".copilot/skills/other");
    assert!(!std::fs::symlink_metadata(&cp).unwrap().file_type().is_symlink());
    assert!(cp.join("SKILL.md").exists());
    // Remove cleans up placements.
    s.ok(&["remove", "hello"]);
    assert!(std::fs::symlink_metadata(&claude).is_err());
}

#[test]
fn review_policy_prepares_update_without_changing_deployment() {
    let s = Sandbox::new();
    let r = hello_repo(&s);
    s.ok(&["add", "acme/skills//hello"]);
    write(&r.join("skills/hello/SKILL.md"), &skill_md("hello", "Say hello. Use when greeting.", "# Hello\n\nv2 https://example.com\n"));
    write(&r.join("skills/hello/scripts/run.sh"), "#!/bin/sh\necho hi\n");
    commit_all(&r, "v2");
    git(&r, &["tag", "v1.1.0"]);
    // Force the fetch interval to elapse.
    std::fs::write(
        s.config.join("tricks.toml"),
        std::fs::read_to_string(s.config.join("tricks.toml")).unwrap().replace("[settings]", "[settings]\nfetch_interval = \"0s\""),
    )
    .unwrap();
    let inst = s.json(&["install"]);
    assert_eq!(inst["updates_ready"], 1, "{inst}");
    // The status line reads cached state only.
    assert_eq!(s.ok(&["statusline"]).trim(), "tricks: 1 update");
    let deployed = std::fs::read_to_string(s.home.join(".claude/skills/hello/SKILL.md")).unwrap();
    assert!(deployed.contains("v1"), "review must not change the deployment");
    let out = s.json(&["outdated"]);
    assert_eq!(out[0]["to_ref"], "v1.1.0");
    let details = out[0]["details"].as_array().unwrap().iter().map(|d| d.as_str().unwrap().to_string()).collect::<Vec<_>>().join("\n");
    assert!(details.contains("+ script scripts/run.sh"), "{details}");
    // Without --yes and without a TTY, update requires confirmation.
    let err = s.fail(&["update"]);
    assert!(err.contains("confirmation required"), "{err}");
    s.ok(&["update", "--yes"]);
    let deployed = std::fs::read_to_string(s.home.join(".claude/skills/hello/SKILL.md")).unwrap();
    assert!(deployed.contains("v2"));
    let lock = std::fs::read_to_string(s.config.join("tricks.lock")).unwrap();
    assert!(lock.contains("ref_name = \"v1.1.0\""));

    // Rollback restores the previous revision and pins it.
    s.ok(&["rollback", "hello"]);
    let deployed = std::fs::read_to_string(s.home.join(".claude/skills/hello/SKILL.md")).unwrap();
    assert!(deployed.contains("v1"));
    let manifest = std::fs::read_to_string(s.config.join("tricks.toml")).unwrap();
    assert!(manifest.contains("update = \"pinned\""), "{manifest}");
    // The next install does not undo the rollback.
    s.ok(&["install"]);
    assert!(std::fs::read_to_string(s.home.join(".claude/skills/hello/SKILL.md")).unwrap().contains("v1"));
}

#[test]
fn auto_policy_needs_ownership_and_never_rewrites_lock() {
    let mut s = Sandbox::new();
    let r =
        s.upstream("acme", "team-skills", &[("skills/deploy/SKILL.md", &skill_md("deploy", "Deploy things. Use when deploying.", "v1\n"))]);
    // Not signed in: auto is refused.
    let err = s.fail(&["add", "acme/team-skills//deploy@main", "--update", "auto"]);
    assert!(err.contains("cannot verify"), "{err}");
    // Third-party owner: refused; unsafe-auto is the explicit escape hatch.
    s.identity = Some("someone,other-org".into());
    let err = s.fail(&["add", "acme/team-skills//deploy@main", "--update", "auto"]);
    assert!(err.contains("unsafe-auto"), "{err}");
    s.identity = Some("me,acme".into());
    s.ok(&["add", "acme/team-skills//deploy@main", "--update", "auto"]);
    let lock_before = std::fs::read_to_string(s.config.join("tricks.lock")).unwrap();

    write(&r.join("skills/deploy/SKILL.md"), &skill_md("deploy", "Deploy things. Use when deploying.", "v2\n"));
    commit_all(&r, "v2");
    std::fs::write(
        s.config.join("tricks.toml"),
        std::fs::read_to_string(s.config.join("tricks.toml")).unwrap().replace("[settings]", "[settings]\nfetch_interval = \"0s\""),
    )
    .unwrap();
    let inst = s.json(&["install"]);
    assert_eq!(inst["skills"][0]["action"], "auto-updated", "{inst}");
    assert!(std::fs::read_to_string(s.home.join(".claude/skills/deploy/SKILL.md")).unwrap().contains("v2"));
    assert_eq!(std::fs::read_to_string(s.config.join("tricks.lock")).unwrap(), lock_before, "auto must not rewrite the lock");
    let st = s.json(&["status"]);
    assert_eq!(st["workbench"]["skills"][0]["ahead_of_lock"], true);
    // --frozen deploys exactly the lock.
    s.ok(&["install", "--frozen"]);
    assert!(std::fs::read_to_string(s.home.join(".claude/skills/deploy/SKILL.md")).unwrap().contains("v1"));
}

#[test]
fn link_into_project_keeps_git_clean_and_shadow_restores() {
    let s = Sandbox::new();
    hello_repo(&s);
    let proj = s.project("app");
    // Existing, unmanaged skill with the same name.
    write(&proj.join(".claude/skills/hello/SKILL.md"), "original\n");
    commit_all(&proj, "vendored skill");
    let err = s.fail(&["link", "acme/skills//hello", "--to", proj.to_str().unwrap(), "--agents", "claude"]);
    assert!(err.contains("--shadow"), "{err}");
    s.ok(&["link", "acme/skills//hello", "--to", proj.to_str().unwrap(), "--agents", "claude,codex", "--shadow"]);
    let st = git(&proj, &["status", "--porcelain"]);
    // The shadowed tracked file shows as a typechange only for the tracked path; the new
    // codex placement must be excluded.
    assert!(!st.contains(".agents"), "untracked placement leaked into git status: {st}");
    let exclude = std::fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains("/.agents/skills/hello"));
    s.ok(&["unlink", "--all"]);
    assert_eq!(std::fs::read_to_string(proj.join(".claude/skills/hello/SKILL.md")).unwrap(), "original\n");
    assert!(git(&proj, &["status", "--porcelain"]).is_empty());
    let exclude = std::fs::read_to_string(proj.join(".git/info/exclude")).unwrap();
    assert!(!exclude.contains("tricks"), "{exclude}");
}

#[test]
fn link_untracked_project_placement_is_invisible_to_git() {
    let s = Sandbox::new();
    hello_repo(&s);
    let proj = s.project("app2");
    s.ok(&["link", "acme/skills//hello", "--to", proj.to_str().unwrap(), "--agents", "claude,cursor"]);
    assert!(git(&proj, &["status", "--porcelain"]).is_empty());
    // A linked worktree shares the exclude file.
    let wt = s.root().join("projects/app2-wt");
    git(&proj, &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "wt"]);
    s.ok(&["link", "acme/skills//hello", "--to", wt.to_str().unwrap(), "--agents", "claude"]);
    assert!(git(&wt, &["status", "--porcelain"]).is_empty());
    s.ok(&["unlink", "hello", "--to", wt.to_str().unwrap()]);
    // Main checkout still excluded because its placement remains.
    assert!(git(&proj, &["status", "--porcelain"]).is_empty());
    s.ok(&["unlink", "--all"]);
}

#[test]
fn follows_upstream_renames() {
    let s = Sandbox::new();
    let r = hello_repo(&s);
    s.ok(&["add", "acme/skills//hello@main"]);
    std::fs::write(
        s.config.join("tricks.toml"),
        std::fs::read_to_string(s.config.join("tricks.toml")).unwrap().replace("[settings]", "[settings]\nfetch_interval = \"0s\""),
    )
    .unwrap();
    // Upstream reorganizes: skills/hello → skills/greetings/hello, and edits it.
    std::fs::create_dir_all(r.join("skills/greetings")).unwrap();
    git(&r, &["mv", "skills/hello", "skills/greetings/hello"]);
    write(&r.join("skills/greetings/hello/SKILL.md"), &skill_md("hello", "Say hello. Use when greeting.", "# Hello\n\nv1\nmoved\n"));
    commit_all(&r, "reorganize");
    let out = s.json(&["outdated"]);
    let details = out[0]["details"].to_string();
    assert!(details.contains("moved upstream: skills/hello → skills/greetings/hello"), "{out}");
    // Update follows the rename: manifest, lock and placements move to the new id.
    s.ok(&["update", "--yes"]);
    let manifest = std::fs::read_to_string(s.config.join("tricks.toml")).unwrap();
    assert!(manifest.contains("github.com/acme/skills//skills/greetings/hello") && !manifest.contains("//skills/hello\""), "{manifest}");
    let lock = std::fs::read_to_string(s.config.join("tricks.lock")).unwrap();
    assert!(lock.contains("//skills/greetings/hello") && !lock.contains("path = "), "{lock}");
    assert!(std::fs::read_to_string(s.home.join(".claude/skills/hello/SKILL.md")).unwrap().contains("moved"));
    let st = s.json(&["status"]);
    assert_eq!(st["workbench"]["skills"][0]["id"], "github.com/acme/skills//skills/greetings/hello");
    assert_eq!(st["workbench"]["skills"][0]["placements"].as_array().unwrap().len(), 2);
}

#[test]
fn agent_skill_install_and_remove() {
    let s = Sandbox::new();
    let st = s.json(&["agent-skill", "--agents", "claude"]);
    assert_eq!(st["paths"].as_array().unwrap().len(), 1);
    let md = std::fs::read_to_string(s.home.join(".claude/skills/new-tricks/SKILL.md")).unwrap();
    assert!(md.contains("name: new-tricks") && md.contains("Bash(tricks search:*)"));
    s.ok(&["agent-skill", "--remove"]);
    assert!(std::fs::symlink_metadata(s.home.join(".claude/skills/new-tricks")).is_err());
}

#[test]
fn starred_trust_facet() {
    let s = Sandbox::new();
    hello_repo(&s);
    s.ok(&["source", "add", "acme/skills"]);
    let o = s.cmd(&s.root(), &["--json", "search", "--no-live", "hello"]);
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(r[0]["trust"], "unknown");
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_tricks"));
    let o = c
        .args(["--json", "search", "--no-live", "--trust", "starred", "hello"])
        .env("TRICKS_HOME", &s.home)
        .env("TRICKS_CONFIG_DIR", &s.config)
        .env("TRICKS_DATA_DIR", &s.data)
        .env("TRICKS_HOST_MAP", format!("github.com={}", s.fixtures.display()))
        .env("TRICKS_NO_GH", "1")
        .env("TRICKS_NO_API", "1")
        .env("TRICKS_STARRED", "acme/skills")
        .output()
        .unwrap();
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(r[0]["trust"], "starred", "{r}");
    assert_eq!(r[0]["name"], "hello");
}
