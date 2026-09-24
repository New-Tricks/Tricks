//! M3/M4 acceptance: workspace authoring, upstream merges, variants, lint, publish, pr.
// Asserts on symlinked placements; Windows deploys copies (spec §8), so these run on Unix.
#![cfg(unix)]

mod common;
use common::*;
use std::path::{Path, PathBuf};

const PARA: &str = "# Hello\n\nIntro paragraph.\n\n## Steps\n\n1. Greet.\n2. Wave.\n\n## Notes\n\nKeep it short.\n";

fn setup() -> (Sandbox, PathBuf, PathBuf) {
    let s = Sandbox::new();
    let up = s.upstream(
        "acme",
        "skills",
        &[
            ("skills/hello/SKILL.md", &skill_md("hello", "Greets people politely. Use when the user asks for a greeting.", PARA)),
            ("skills/hello/LICENSE", "MIT License\n\nCopyright (c) 2024 Acme\n\nPermission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the \"Software\"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:\n\nThe above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.\n\nTHE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.\n"),
            ("skills/secret-sauce/SKILL.md", "---\nname: secret-sauce\ndescription: Proprietary recipe helper. Use when cooking the secret sauce.\nlicense: Proprietary. LICENSE.txt has complete terms\n---\nbody\n"),
            ("skills/secret-sauce/LICENSE.txt", "© 2025 Acme. All rights reserved. You may not distribute or create derivative works.\n"),
        ],
    );
    git(&up, &["tag", "v1.0.0"]);
    let ws = s.root().join("my-skills");
    std::fs::create_dir_all(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    s.ok_in(&ws, &["init"]);
    commit_all(&ws, "init workspace");
    (s, up, ws)
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn set_interval_zero(s: &Sandbox) {
    let p = s.config.join("tricks.toml");
    let t = read(&p);
    if !t.contains("fetch_interval") {
        std::fs::write(&p, t.replace("[settings]", "[settings]\nfetch_interval = \"0s\"")).unwrap();
    }
}

#[test]
fn init_vendor_and_block_class_confirmation() {
    let (s, _up, ws) = setup();
    assert!(read(&ws.join(".gitignore")).contains("tricks.work.toml"));
    assert!(read(&s.config.join("tricks.toml")).contains("[workspaces]"));
    let r = s.json_in(&ws, &["vendor", "acme/skills//hello"]);
    assert_eq!(r["upstream"], "github.com/acme/skills//skills/hello");
    assert_eq!(r["license"]["class"], "allow");
    assert!(ws.join("skills/hello/SKILL.md").exists());
    let m = read(&ws.join("tricks.toml"));
    assert!(m.contains("[skills.hello]") && m.contains("upstream = \"github.com/acme/skills//skills/hello\""), "{m}");
    assert!(read(&ws.join("tricks.lock")).contains("base = "));
    // Proprietary skill: vendoring needs explicit confirmation.
    let err = s.fail_in(&ws, &["vendor", "acme/skills//secret-sauce"]);
    assert!(err.contains("confirmation required") && err.contains("may prohibit"), "{err}");
    s.ok_in(&ws, &["vendor", "acme/skills//secret-sauce", "--yes"]);
    // Search/show report vendored state.
    let sh = s.json_in(&ws, &["show", "acme/skills//hello"]);
    assert_eq!(sh["vendored"], true);
}

#[test]
fn clean_merge_preserves_customization_and_is_left_uncommitted() {
    let (s, up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//hello"]);
    commit_all(&ws, "vendor hello");
    // Customize the intro paragraph.
    let p = ws.join("skills/hello/SKILL.md");
    std::fs::write(&p, read(&p).replace("Intro paragraph.", "My custom intro.")).unwrap();
    commit_all(&ws, "customize intro");
    // Dev-link it so we can check that deployments stay on the committed version.
    s.ok_in(&ws, &["install", "--agents", "claude"]);
    let deployed = s.home.join(".claude/skills/hello/SKILL.md");
    assert!(read(&deployed).contains("My custom intro."));
    // Upstream changes a different paragraph and adds a file.
    let up_md = up.join("skills/hello/SKILL.md");
    std::fs::write(&up_md, read(&up_md).replace("Keep it short.", "Keep it short and friendly.")).unwrap();
    write(&up.join("skills/hello/references/tips.md"), "tips\n");
    commit_all(&up, "upstream improvements");
    git(&up, &["tag", "v1.1.0"]);
    set_interval_zero(&s);
    let out = s.json_in(&ws, &["outdated"]);
    assert_eq!(out["items"][0]["state"], "update-available", "{out}");
    // C → R preview: shows what the merge would produce without touching the working tree.
    let cand = s.json_in(&ws, &["diff", "hello", "--from", "working", "--to", "candidate"]).to_string();
    assert!(cand.contains("friendly") && cand.contains("tips.md"), "{cand}");
    assert!(git(&ws, &["status", "--porcelain"]).is_empty(), "candidate must not modify the working tree");
    let r = s.json_in(&ws, &["update", "hello"]);
    assert_eq!(r["items"][0]["state"], "merged", "{r}");
    let merged = read(&p);
    assert!(merged.contains("My custom intro.") && merged.contains("Keep it short and friendly."), "{merged}");
    assert!(ws.join("skills/hello/references/tips.md").exists());
    // Left uncommitted, lock base bumped.
    assert!(!git(&ws, &["status", "--porcelain"]).is_empty());
    let up_head = git(&up, &["rev-parse", "HEAD"]);
    assert!(read(&ws.join("tricks.lock")).contains(&up_head));
    // Agents keep the committed version until the merge is committed.
    assert!(!read(&deployed).contains("friendly"), "deployment changed before commit");
    s.ok_in(&ws, &["commit", "hello", "-m", "merge upstream v1.1.0"]);
    assert!(read(&deployed).contains("friendly"), "deployment did not resume after commit");
    // Diff views: B→C shows only my customization.
    let d = s.json_in(&ws, &["diff", "hello", "--from", "base", "--to", "working"]);
    let text = d.to_string();
    assert!(text.contains("My custom intro") && !text.contains("friendly"), "{text}");
}

#[test]
fn overlapping_change_conflicts_then_continue_or_abort() {
    let (s, up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//hello"]);
    let p = ws.join("skills/hello/SKILL.md");
    std::fs::write(&p, read(&p).replace("Keep it short.", "Keep it VERY short.")).unwrap();
    commit_all(&ws, "vendor + customize");
    let up_md = up.join("skills/hello/SKILL.md");
    std::fs::write(&up_md, read(&up_md).replace("Keep it short.", "Keep it brief.")).unwrap();
    commit_all(&up, "conflicting upstream");
    git(&up, &["tag", "v1.1.0"]);
    set_interval_zero(&s);
    let lock_before = read(&ws.join("tricks.lock"));
    let r = s.json_in(&ws, &["update"]);
    assert_eq!(r["items"][0]["state"], "conflicts", "{r}");
    assert!(read(&p).contains("<<<<<<<"));
    assert_eq!(read(&ws.join("tricks.lock")), lock_before, "base must not move while conflicted");
    // Another update refuses while a merge is in progress.
    let err = s.fail_in(&ws, &["update"]);
    assert!(err.contains("in progress"), "{err}");
    // --continue refuses with markers present.
    let err = s.fail_in(&ws, &["update", "--continue"]);
    assert!(err.contains("unresolved"), "{err}");
    // Abort restores my version.
    s.ok_in(&ws, &["update", "--abort"]);
    assert!(read(&p).contains("Keep it VERY short.") && !read(&p).contains("<<<<<<<"));
    // Redo and resolve.
    s.ok_in(&ws, &["update"]);
    let resolved = read(&p)
        .lines()
        .filter(|l| !l.starts_with("<<<<<<<") && !l.starts_with("=======") && !l.starts_with(">>>>>>>") && !l.contains("Keep it brief."))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&p, resolved).unwrap();
    s.ok_in(&ws, &["update", "--continue"]);
    assert!(read(&ws.join("tricks.lock")).contains(&git(&up, &["rev-parse", "HEAD"])));
}

#[test]
fn branch_experiments_variants_and_agent_trailers() {
    let (s, _up, ws) = setup();
    s.ok_in(
        &ws,
        &["new", "greeter", "--description", "Writes greetings for any occasion. Use when the user asks for a greeting card text."],
    );
    commit_all(&ws, "new greeter");
    s.ok_in(&ws, &["install", "--agents", "claude"]);
    let deployed = s.home.join(".claude/skills/greeter");
    let target = std::fs::read_link(&deployed).unwrap();
    assert_eq!(target, ws.join("skills/greeter"), "dev link points at the live checkout");
    // Edit on a branch → worktree, dev link flips to it.
    let out = s.json_in(&ws, &["edit", "greeter", "--branch", "terse"]);
    let path = PathBuf::from(out["path"].as_str().unwrap());
    assert!(path.join("SKILL.md").exists());
    assert_eq!(std::fs::read_link(&deployed).unwrap(), path);
    std::fs::write(path.join("SKILL.md"), read(&path.join("SKILL.md")).replace("## Instructions", "## Instructions (terse)")).unwrap();
    // Commit as an agent.
    let o = {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_tricks"));
        c.args(["--json", "commit", "greeter", "-m", "terse variant"])
            .current_dir(&ws)
            .env("TRICKS_HOME", &s.home)
            .env("TRICKS_CONFIG_DIR", &s.config)
            .env("TRICKS_DATA_DIR", &s.data)
            .env("TRICKS_NO_GH", "1")
            .env("CLAUDECODE", "1")
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@e")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@e");
        c.output().unwrap()
    };
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let msg = git(&ws, &["log", "-1", "--format=%B", "terse"]);
    assert!(msg.contains("Tricks-Agent: claude-code"), "{msg}");
    // Back on the main checkout.
    assert_eq!(std::fs::read_link(&deployed).unwrap(), ws.join("skills/greeter"));
    // Use the variant: store snapshot of the branch tip.
    s.ok_in(&ws, &["use", "greeter@terse"]);
    let t = std::fs::read_link(&deployed).unwrap();
    assert!(t.starts_with(&s.data), "variant deploys from the store: {}", t.display());
    assert!(read(&deployed.join("SKILL.md")).contains("(terse)"));
    assert!(read(&ws.join("tricks.toml")).contains("use = \"terse\""));
    // Local override back to default without touching the committed manifest.
    s.ok_in(&ws, &["use", "greeter@default", "--local"]);
    assert!(read(&ws.join("tricks.work.toml")).contains("greeter = \"default\""));
    assert_eq!(std::fs::read_link(&deployed).unwrap(), ws.join("skills/greeter"));
    assert!(git(&ws, &["status", "--porcelain", "--", "tricks.work.toml"]).is_empty(), "work file must be gitignored");
    let st = s.json_in(&ws, &["status"]);
    let skills = st["workspace"]["skills"].as_array().unwrap();
    let g = skills.iter().find(|x| x["name"] == "greeter").unwrap();
    assert!(g["branches"].as_array().unwrap().iter().any(|b| b == "terse"), "{g}");
}

#[test]
fn lint_blocks_publish_and_publish_generates_ecosystem_files() {
    let (s, _up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//hello"]);
    s.ok_in(
        &ws,
        &["new", "greeter", "--description", "Writes greetings for any occasion. Use when the user asks for a greeting card text."],
    );
    write(&ws.join("skills/greeter/notes/ideas.md"), "private notes\n");
    write(
        &ws.join("skills/greeter/SKILL.md"),
        &read(&ws.join("skills/greeter/SKILL.md")).replace("---\n\n#", "metadata:\n  tricks-lint-disable: NT203\n---\n\n#"),
    );
    let target = s.root().join("acme-skills-public");
    std::fs::create_dir_all(&target).unwrap();
    git(&target, &["init", "-q", "-b", "main"]);
    write(&target.join("README.md"), "hand-written readme\n");
    commit_all(&target, "readme");
    let m = read(&ws.join("tricks.toml"));
    std::fs::write(
        ws.join("tricks.toml"),
        format!("{m}\n[publish.targets.public]\nrepo = \"../acme-skills-public\"\nskills = [\"*\"]\nexclude = [\"notes/**\"]\n"),
    )
    .unwrap();
    commit_all(&ws, "setup");

    // A lint error blocks publishing.
    let md = ws.join("skills/greeter/SKILL.md");
    let good = read(&md);
    std::fs::write(&md, good.replace("name: greeter", "name: Greeter")).unwrap();
    commit_all(&ws, "break name");
    let r = s.json_any_in(&ws, &["publish", "public", "--dry-run"]);
    assert_eq!(r["blocked"], true);
    assert!(r["gates"].to_string().contains("NT102"), "{}", r["gates"]);
    std::fs::write(&md, good).unwrap();
    commit_all(&ws, "fix name");

    // Dirty workspace blocks a real publish.
    write(&ws.join("scratch.txt"), "x");
    let o = s.cmd(&ws, &["--json", "publish", "public", "--bump", "minor", "--yes"]);
    assert!(!o.status.success());
    std::fs::remove_file(ws.join("scratch.txt")).unwrap();

    let r = s.json_in(&ws, &["publish", "public", "--bump", "minor", "--yes"]);
    assert_eq!(r["blocked"], false, "{r}");
    assert_eq!(r["version"], "0.1.0");
    assert_eq!(r["tag"], "v0.1.0");
    assert!(target.join("skills/hello/SKILL.md").exists());
    assert!(target.join("skills/hello/LICENSE").exists(), "upstream licence carried");
    assert!(!target.join("skills/greeter/notes").exists(), "exclude globs applied");
    let greeter = read(&target.join("skills/greeter/SKILL.md"));
    assert!(greeter.contains("version: \"0.1.0\"") && !greeter.contains("tricks-lint-disable"), "{greeter}");
    let mp: serde_json::Value = serde_json::from_str(&read(&target.join(".claude-plugin/marketplace.json"))).unwrap();
    assert_eq!(mp["name"], "acme-skills-public");
    assert_eq!(mp["plugins"].as_array().unwrap().len(), 1, "single plugin by default");
    assert_eq!(mp["plugins"][0]["source"], "./");
    assert_eq!(mp["plugins"][0]["strict"], false);
    assert!(read(&target.join("apm.yml")).contains("version: 0.1.0"));
    assert!(read(&target.join("PROVENANCE.md")).contains("github.com/acme/skills//skills/hello"));
    assert!(read(&target.join("CHANGELOG.md")).contains("## v0.1.0"));
    let msg = git(&target, &["log", "-1", "--format=%B"]);
    assert!(msg.contains("Tricks-Source:"), "{msg}");
    assert_eq!(git(&target, &["tag", "--list"]), "v0.1.0");

    // Re-publish after deselecting a skill: removes it, keeps hand-added files.
    let m = read(&ws.join("tricks.toml")).replace("skills = [\"*\"]", "skills = [\"greeter\"]");
    std::fs::write(ws.join("tricks.toml"), m + "\n[publish.targets.public.plugins]\n").unwrap();
    commit_all(&ws, "only greeter");
    let r = s.json_in(&ws, &["publish", "public", "--yes"]);
    assert_eq!(r["suggested_bump"], "major", "{r}");
    assert!(!target.join("skills/hello").exists());
    assert_eq!(read(&target.join("README.md")), "hand-written readme\n");
}

#[test]
fn licence_gate_blocks_proprietary_vendored_skill_unless_overridden() {
    let (s, _up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//secret-sauce", "--yes"]);
    let target = s.root().join("pub");
    std::fs::create_dir_all(&target).unwrap();
    git(&target, &["init", "-q", "-b", "main"]);
    write(&target.join("README.md"), "r\n");
    commit_all(&target, "r");
    let m = read(&ws.join("tricks.toml"));
    std::fs::write(ws.join("tricks.toml"), format!("{m}\n[publish.targets.public]\nrepo = \"../pub\"\nmarketplace = \"acme-pub\"\n"))
        .unwrap();
    commit_all(&ws, "setup");
    let r = s.json_any_in(&ws, &["publish", "public", "--dry-run"]);
    assert_eq!(r["blocked"], true);
    let lic = r["gates"].as_array().unwrap().iter().find(|g| g["name"] == "licence").unwrap().clone();
    assert_eq!(lic["status"], "fail", "{lic}");
    s.ok_in(&ws, &["allow-license", "secret-sauce", "We hold a separate redistribution agreement with Acme."]);
    commit_all(&ws, "override");
    let r = s.json_in(&ws, &["publish", "public", "--dry-run"]);
    let lic = r["gates"].as_array().unwrap().iter().find(|g| g["name"] == "licence").unwrap().clone();
    assert_eq!(lic["status"], "warn", "{lic}");
}

#[test]
fn pr_dry_run_contains_only_the_customization() {
    let (s, up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//hello"]);
    let p = ws.join("skills/hello/SKILL.md");
    std::fs::write(&p, read(&p).replace("2. Wave.", "2. Wave.\n3. Smile.")).unwrap();
    commit_all(&ws, "add a step");
    // Upstream moved on meanwhile (non-overlapping).
    let up_md = up.join("skills/hello/SKILL.md");
    std::fs::write(&up_md, read(&up_md).replace("Intro paragraph.", "Intro paragraph, revised.")).unwrap();
    commit_all(&up, "upstream edit");
    let r = s.json_in(&ws, &["pr", "hello", "--dry-run"]);
    assert_eq!(r["files"].as_array().unwrap().len(), 1, "{r}");
    let wt = PathBuf::from(r["worktree"].as_str().unwrap());
    let content = read(&wt.join("skills/hello/SKILL.md"));
    assert!(content.contains("3. Smile.") && content.contains("revised"), "{content}");
    let diff = git(&wt, &["show", "--stat", "HEAD"]);
    assert!(diff.contains("skills/hello/SKILL.md") && !diff.contains("tricks"), "{diff}");
}
