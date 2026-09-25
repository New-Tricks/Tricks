//! Links: source repo skills and upstream trials deployed for testing, git hygiene,
//! shadowing, and cleanup of user-scope installs left by 0.2.
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

fn is_link(p: &std::path::Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

#[test]
fn short_id_and_url_resolve_to_same_canonical() {
    let s = Sandbox::new();
    let r = hello_repo(&s);
    let a = s.json(&["show", "acme/skills//hello"]);
    let b = s.json(&["show", "https://github.com/acme/skills/tree/v1.0.0/skills/hello"]);
    let c = s.json(&["info", "acme/skills//skills/hello@v1.0.0"]);
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
fn trying_an_upstream_skill_links_it_into_the_current_project() {
    let s = Sandbox::new();
    hello_repo(&s);
    let proj = s.project("app");
    let r = s.json_in(&proj, &["link", "acme/skills//hello"]);
    assert_eq!(r["links"][0]["trial"], true, "{r}");
    assert_eq!(r["links"][0]["scope"], proj.to_string_lossy().as_ref());
    let claude = proj.join(".claude/skills/hello");
    assert!(is_link(&claude));
    assert!(proj.join(".agents/skills/hello/SKILL.md").exists(), "codex placement");
    // Store entries are read-only, and nothing shows up in git.
    let target = std::fs::read_link(&claude).unwrap();
    assert!(std::fs::metadata(target.join("SKILL.md")).unwrap().permissions().readonly());
    assert!(git(&proj, &["status", "--porcelain"]).is_empty());
    // Nothing was written to user-level agent directories or the user config.
    assert!(!s.home.join(".claude/skills/hello").exists());
    assert!(!std::fs::read_to_string(s.config.join("tricks.toml")).unwrap().contains("hello"));
    // Copilot always gets a copy (microsoft/vscode#315979).
    s.ok_in(&proj, &["link", "acme/skills//other", "--agents", "copilot"]);
    let cp = proj.join(".github/skills/other");
    assert!(!is_link(&cp) && cp.join("SKILL.md").exists());
    // Status and show report the trial.
    let st = s.json(&["status"]);
    let links = st["links"].as_array().unwrap();
    assert_eq!(links.len(), 3, "{st}");
    assert!(links.iter().all(|l| l["kind"] == "trial" && l["health"] == "ok"), "{st}");
    assert_eq!(s.json(&["show", "acme/skills//hello"])["linked"], true);
    // Unlinking removes the placements and prunes the store.
    let store = s.data.join("store");
    let before = std::fs::read_dir(&store).unwrap().count();
    s.ok_in(&proj, &["unlink", "hello"]);
    assert!(std::fs::symlink_metadata(&claude).is_err());
    assert!(std::fs::read_dir(&store).unwrap().count() < before, "unlink prunes unreferenced store entries");
    s.ok(&["unlink", "--all"]);
    assert!(s.json(&["status"])["links"].as_array().unwrap().is_empty());
}

#[test]
fn repo_skills_link_globally_in_dev_mode_and_unlink_together() {
    let s = Sandbox::new();
    let ws = s.project("my-skills");
    s.ok_in(&ws, &["init"]);
    s.ok_in(&ws, &["new", "greeter", "--description", "Greets people. Use when the user asks for a greeting."]);
    s.ok_in(&ws, &["new", "farewell", "--description", "Says goodbye. Use when the user is leaving."]);
    let r = s.json_in(&ws, &["link"]);
    assert_eq!(r["links"].as_array().unwrap().len(), 2, "{r}");
    assert!(r["links"].as_array().unwrap().iter().all(|l| l["scope"] == "global" && l["trial"] == false));
    let deployed = s.home.join(".claude/skills/greeter");
    assert!(is_link(&deployed));
    // Dev mode: edits in the working tree are live.
    std::fs::write(ws.join("skills/greeter/SKILL.md"), "---\nname: greeter\ndescription: Greets. Use when greeting.\n---\nlive edit\n")
        .unwrap();
    assert!(std::fs::read_to_string(deployed.join("SKILL.md")).unwrap().contains("live edit"));
    let st = s.json_in(&ws, &["status"]);
    assert_eq!(st["source_repo"]["skills"].as_array().unwrap().len(), 2);
    assert!(st["links"].as_array().unwrap().iter().all(|l| l["kind"] == "dev"));
    assert_eq!(s.ok_in(&ws, &["statusline"]).trim(), "tricks: 4 links");
    // One skill into a project instead.
    let proj = s.project("app");
    s.ok_in(&ws, &["link", "greeter", "--to", proj.to_str().unwrap(), "--agents", "claude"]);
    assert!(is_link(&proj.join(".claude/skills/greeter")));
    // `unlink` with no skill removes every link of this repo's skills.
    s.ok_in(&ws, &["unlink"]);
    assert!(std::fs::symlink_metadata(&deployed).is_err());
    assert!(std::fs::symlink_metadata(proj.join(".claude/skills/greeter")).is_err());
    assert_eq!(s.ok_in(&ws, &["statusline"]).trim(), "");
    // Outside a repo, `link` needs a skill.
    let err = s.fail(&["link"]);
    assert!(err.contains("inside a source repo"), "{err}");
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
fn user_scope_installs_from_0_2_are_reported_and_removable() {
    let s = Sandbox::new();
    // What New Tricks 0.2 left behind: a placement at user scope, a lock and a [skills] table.
    let target = s.data.join("store/0123abcd");
    write(&target.join("SKILL.md"), &skill_md("old", "An old install. Use when testing.", "old\n"));
    let dest = s.home.join(".claude/skills/old");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &dest).unwrap();
    let state = newtricks::state::State::open(&s.data.join("state.db")).unwrap();
    state
        .insert_placement(&newtricks::state::Placement {
            id: 0,
            skill: "github.com/acme/skills//skills/old".into(),
            origin: "user".into(),
            agent: "claude".into(),
            scope: "global".into(),
            path: dest.to_string_lossy().into(),
            mode: "link".into(),
            target: target.to_string_lossy().into(),
            tree: Some("0123abcd".into()),
            commit: None,
            shadow_backup: None,
            exclude_file: None,
            exclude_entry: None,
            created_at: 0,
        })
        .unwrap();
    drop(state);
    let cfg = s.config.join("tricks.toml");
    std::fs::write(&cfg, format!("{}\n[skills]\n\"github.com/acme/skills//skills/old\" = {{ version = \"latest\" }}\n", read(&cfg)))
        .unwrap();

    assert_eq!(s.json(&["status"])["legacy"], 1);
    let d = s.json(&["doctor"]);
    let check = d["checks"].as_array().unwrap().iter().find(|c| c["name"] == "user-scope installs").unwrap_or_else(|| panic!("{d}"));
    assert!(check["detail"].as_str().unwrap().contains("tricks unlink --legacy"), "{check}");
    // `unlink --all` only touches links; `--legacy` removes the old install.
    s.ok(&["unlink", "--all"]);
    assert!(is_link(&dest));
    s.ok(&["unlink", "--legacy"]);
    assert!(std::fs::symlink_metadata(&dest).is_err());
    assert_eq!(s.json(&["status"])["legacy"], 0);
}

#[test]
fn starred_trust_facet() {
    let s = Sandbox::new();
    hello_repo(&s);
    s.ok(&["catalog", "add", "acme/skills"]);
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
