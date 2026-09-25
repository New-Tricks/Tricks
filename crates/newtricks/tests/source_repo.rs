//! M3/M4 acceptance: source repo authoring, upstream merges, variants, lint, publish, contribute.
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
    commit_all(&ws, "init source repo");
    (s, up, ws)
}

/// A bare distribution repository at `github.com/<owner>/<name>` holding one README.
fn publish_remote(s: &Sandbox, owner: &str, name: &str, readme: &str) -> PathBuf {
    let remote = s.fixtures.join(owner).join(name);
    std::fs::create_dir_all(&remote).unwrap();
    git(&remote, &["init", "-q", "--bare", "-b", "main"]);
    let seed = s.root().join(format!("seed-{name}"));
    git(&s.root(), &["clone", "-q", remote.to_str().unwrap(), seed.to_str().unwrap()]);
    write(&seed.join("README.md"), readme);
    commit_all(&seed, "readme");
    git(&seed, &["push", "-q", "origin", "HEAD:main"]);
    remote
}

/// A fresh clone of a bare remote, to inspect what was pushed.
fn checkout(s: &Sandbox, remote: &Path) -> PathBuf {
    let d = tempfile::Builder::new().prefix("check-").tempdir_in(s.root()).unwrap().keep();
    git(&s.root(), &["clone", "-q", remote.to_str().unwrap(), d.join("c").to_str().unwrap()]);
    d.join("c")
}

fn set_interval_zero(s: &Sandbox) {
    let p = s.config.join("tricks.toml");
    let t = read(&p);
    if !t.contains("fetch_interval") {
        std::fs::write(&p, t.replace("[settings]", "[settings]\nfetch_interval = \"0s\"")).unwrap();
    }
}

#[test]
fn init_agent_skill_is_placed_in_the_repository_not_user_scope() {
    let s = Sandbox::new();
    let ws = s.root().join("team-skills");
    std::fs::create_dir_all(&ws).unwrap();
    git(&ws, &["init", "-q", "-b", "main"]);
    let r = s.json_in(&ws, &["init", "--agent-skill"]);
    let placed: Vec<String> = r["agent_skill"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
    assert_eq!(placed.len(), 2, "{r}");
    assert!(placed.iter().all(|p| p.starts_with(ws.to_str().unwrap())), "{placed:?}");
    assert!(ws.join(".claude/skills/new-tricks/SKILL.md").exists());
    assert!(!s.home.join(".claude/skills/new-tricks").exists(), "nothing at user scope");
    // Git-excluded, so the repository stays clean apart from what init itself wrote.
    let status = git(&ws, &["status", "--porcelain", "--untracked-files=all"]);
    assert!(!status.contains("new-tricks"), "{status}");
}

#[test]
fn init_vendor_and_block_class_confirmation() {
    let (s, _up, ws) = setup();
    assert!(read(&ws.join(".gitignore")).contains("tricks.work.toml"));
    assert!(read(&s.config.join("tricks.toml")).contains("[source-repos]"));
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
    let sh = s.json_in(&ws, &["info", "acme/skills//hello"]);
    assert_eq!(sh["vendored"], true);
    // A copy of the upstream skill made earlier: vendored from that copy, at its base.
    let base = git(&_up, &["rev-parse", "v1.0.0"]);
    let copy = s.root().join("elsewhere/hello-copy");
    write(&copy.join("SKILL.md"), &read(&ws.join("skills/hello/SKILL.md")).replace("Intro paragraph.", "My old copy."));
    let r = s.json_in(&ws, &["vendor", "acme/skills//hello", "--name", "hello-copy", "--from", copy.to_str().unwrap(), "--base", &base]);
    assert_eq!(r["name"], "hello-copy", "{r}");
    assert_eq!(r["upstream"], "github.com/acme/skills//skills/hello");
    assert_eq!(r["base"], base.as_str());
    assert!(read(&ws.join("skills/hello-copy/SKILL.md")).contains("My old copy."));
    // Folders are not vendored: they are created as local originals.
    let err = s.fail_in(&ws, &["vendor", copy.to_str().unwrap()]);
    assert!(err.contains("tricks create"), "{err}");
    let c = s.json_in(&ws, &["create", "mine", "--from", copy.to_str().unwrap()]);
    assert_eq!(c["upstream"], serde_json::Value::Null, "{c}");
    assert!(read(&ws.join("tricks.toml")).contains("[skills.mine]"));
    // Removing a skill drops its folder, manifest entry and lock entry.
    s.ok_in(&ws, &["remove", "mine", "--yes"]);
    assert!(!ws.join("skills/mine").exists());
    assert!(!read(&ws.join("tricks.toml")).contains("[skills.mine]"));
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
    s.ok_in(&ws, &["link", "--agents", "claude"]);
    let deployed = s.home.join(".claude/skills/hello/SKILL.md");
    assert!(read(&deployed).contains("My custom intro."));
    // Upstream changes a different paragraph and adds a file.
    let up_md = up.join("skills/hello/SKILL.md");
    std::fs::write(&up_md, read(&up_md).replace("Keep it short.", "Keep it short and friendly.")).unwrap();
    write(&up.join("skills/hello/references/tips.md"), "tips\n");
    commit_all(&up, "upstream improvements");
    git(&up, &["tag", "v1.1.0"]);
    set_interval_zero(&s);
    let out = s.json_in(&ws, &["outdated", "--diff"]);
    assert_eq!(out["items"][0]["state"], "update-available", "{out}");
    assert!(out["items"][0]["incoming"].to_string().contains("tips.md"), "{out}");
    assert!(out["diffs"].to_string().contains("Keep it short and friendly."), "incoming diff: {out}");
    assert!(git(&ws, &["status", "--porcelain"]).is_empty(), "outdated changes nothing");
    let st = s.json_in(&ws, &["list"]);
    assert_eq!(st["source_repo"]["skills"][0]["update_available"], "v1.1.0", "{st}");
    // C → R preview: shows what the merge would produce without touching the working tree.
    let cand = s.json_in(&ws, &["sync", "hello", "--dry-run"]).to_string();
    assert!(cand.contains("friendly") && cand.contains("tips.md"), "{cand}");
    assert!(git(&ws, &["status", "--porcelain"]).is_empty(), "candidate must not modify the working tree");
    let r = s.json_in(&ws, &["sync", "hello"]);
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
    s.ok_in(&ws, &["list"]);
    assert!(!read(&deployed).contains("friendly"), "still on the committed version while uncommitted");
    // Commit with plain git; the next tricks command returns agents to the working tree.
    commit_all(&ws, "merge upstream v1.1.0");
    s.ok_in(&ws, &["list"]);
    assert!(read(&deployed).contains("friendly"), "deployment did not resume after commit");
    // Diff views: B→C shows only my customization.
    let d = s.json_in(&ws, &["diff", "hello", "base.."]);
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
    let r = s.json_in(&ws, &["sync"]);
    assert_eq!(r["items"][0]["state"], "conflicts", "{r}");
    assert!(read(&p).contains("<<<<<<<"));
    assert_eq!(read(&ws.join("tricks.lock")), lock_before, "base must not move while conflicted");
    // Another merge refuses while one is in progress.
    let err = s.fail_in(&ws, &["sync"]);
    assert!(err.contains("in progress"), "{err}");
    // --continue refuses with markers present.
    let err = s.fail_in(&ws, &["sync", "--continue"]);
    assert!(err.contains("unresolved"), "{err}");
    // Abort restores my version.
    s.ok_in(&ws, &["sync", "--abort"]);
    assert!(read(&p).contains("Keep it VERY short.") && !read(&p).contains("<<<<<<<"));
    // Redo and resolve.
    s.ok_in(&ws, &["sync"]);
    let resolved = read(&p)
        .lines()
        .filter(|l| !l.starts_with("<<<<<<<") && !l.starts_with("=======") && !l.starts_with(">>>>>>>") && !l.contains("Keep it brief."))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&p, resolved).unwrap();
    s.ok_in(&ws, &["sync", "--continue"]);
    assert!(read(&ws.join("tricks.lock")).contains(&git(&up, &["rev-parse", "HEAD"])));
}

#[test]
fn branch_experiments_and_variants() {
    let (s, _up, ws) = setup();
    s.ok_in(
        &ws,
        &["create", "greeter", "--description", "Writes greetings for any occasion. Use when the user asks for a greeting card text."],
    );
    commit_all(&ws, "new greeter");
    // --commit needs an experiment under way.
    let err = s.fail_in(&ws, &["edit", "greeter", "--commit", "-m", "x"]);
    assert!(err.contains("not being edited"), "{err}");
    // Without -b, edit starts (or continues) draft/<skill>, checked out inside the repo.
    let d = s.json_in(&ws, &["edit", "greeter"]);
    assert_eq!(d["branch"], "draft/greeter", "{d}");
    assert!(PathBuf::from(d["path"].as_str().unwrap()).starts_with(ws.join(".tricks/work")), "{d}");
    assert!(read(&ws.join(".gitignore")).contains("/.tricks/"));
    assert!(git(&ws, &["status", "--porcelain"]).is_empty(), "worktrees are ignored (init added /.tricks/)");
    // `cd "$(tricks edit greeter)"`: stdout is just the path; commands inside it act on the repo.
    let path = s.ok_in(&ws, &["edit", "greeter"]);
    assert_eq!(path.trim(), d["path"].as_str().unwrap());
    let inside = s.json_in(Path::new(path.trim()), &["list"]);
    assert_eq!(inside["source_repo"]["root"], ws.to_str().unwrap(), "{inside}");
    s.ok_in(&ws, &["edit", "greeter", "--done"]);
    // --shell runs $SHELL in the draft.
    let fake = s.root().join("fake-shell.sh");
    let out = s.root().join("shell-out.txt");
    std::fs::write(&fake, format!("#!/bin/sh\npwd > {}\necho \"$TRICKS_EDITING\" >> {}\n", out.display(), out.display())).unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let mut s = s;
    s.env.push(("SHELL".into(), fake.to_string_lossy().into()));
    s.ok_in(&ws, &["edit", "greeter", "--shell"]);
    let recorded = read(&out);
    assert!(recorded.contains(".tricks/work/draft--greeter/skills/greeter") && recorded.contains("greeter@draft/greeter"), "{recorded}");
    s.ok_in(&ws, &["edit", "greeter", "--done"]);
    s.ok_in(&ws, &["link", "--agents", "claude"]);
    let deployed = s.home.join(".claude/skills/greeter");
    let target = std::fs::read_link(&deployed).unwrap();
    assert_eq!(target, ws.join("skills/greeter"), "dev link points at the live checkout");
    // Edit on a branch → worktree, dev link flips to it.
    let out = s.json_in(&ws, &["edit", "greeter", "--branch", "terse"]);
    let path = PathBuf::from(out["path"].as_str().unwrap());
    let worktree = PathBuf::from(out["worktree"].as_str().unwrap());
    assert!(path.join("SKILL.md").exists());
    assert_eq!(std::fs::read_link(&deployed).unwrap(), path);
    std::fs::write(path.join("SKILL.md"), read(&path.join("SKILL.md")).replace("## Instructions", "## Instructions (terse)")).unwrap();
    let c = s.json_in(&ws, &["edit", "greeter", "--commit", "-m", "terse variant"]);
    assert_eq!(c["branch"], "terse", "{c}");
    assert!(git(&worktree, &["status", "--porcelain"]).is_empty());
    let _ = worktree;
    let done = s.json_in(&ws, &["edit", "greeter", "--done"]);
    assert_eq!(done["branch"], "terse", "{done}");
    let err = s.fail_in(&ws, &["edit", "greeter", "--done"]);
    assert!(err.contains("not being edited"), "{err}");
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
    let st = s.json_in(&ws, &["list"]);
    let skills = st["source_repo"]["skills"].as_array().unwrap();
    let g = skills.iter().find(|x| x["name"] == "greeter").unwrap();
    assert!(g["branches"].as_array().unwrap().iter().any(|b| b == "terse"), "{g}");
}

#[test]
fn lint_blocks_publish_and_publish_generates_ecosystem_files() {
    let (s, _up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//hello"]);
    s.ok_in(
        &ws,
        &["create", "greeter", "--description", "Writes greetings for any occasion. Use when the user asks for a greeting card text."],
    );
    write(&ws.join("skills/greeter/notes/ideas.md"), "private notes\n");
    write(
        &ws.join("skills/greeter/SKILL.md"),
        &read(&ws.join("skills/greeter/SKILL.md")).replace("---\n\n#", "metadata:\n  tricks-lint-disable: NT203\n---\n\n#"),
    );
    let remote = publish_remote(&s, "acme", "acme-skills-public", "hand-written readme\n");
    let m = read(&ws.join("tricks.toml"));
    std::fs::write(
        ws.join("tricks.toml"),
        format!("{m}\n[publish.targets.public]\nrepo = \"acme/acme-skills-public\"\nskills = [\"*\"]\nexclude = [\"notes/**\"]\n"),
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

    // Dirty source repo blocks a real publish.
    write(&ws.join("scratch.txt"), "x");
    let o = s.cmd(&ws, &["--json", "publish", "public", "--bump", "minor", "--push", "--yes"]);
    assert!(!o.status.success());
    std::fs::remove_file(ws.join("scratch.txt")).unwrap();

    // A real publish has to say where the result goes.
    let err = s.fail_in(&ws, &["publish", "public", "--bump", "minor", "--yes"]);
    assert!(err.contains("--push") && err.contains("--pr"), "{err}");

    let r = s.json_in(&ws, &["publish", "public", "--bump", "minor", "--push", "--yes"]);
    assert_eq!(r["blocked"], false, "{r}");
    assert_eq!(r["version"], "0.1.0");
    assert_eq!(r["tag"], "v0.1.0");
    assert_eq!(r["pushed"], true);
    let target = checkout(&s, &remote);
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
    assert_eq!(git(&remote, &["tag", "--list"]), "v0.1.0");

    // Re-publish after deselecting a skill: removes it, keeps hand-added files.
    let m = read(&ws.join("tricks.toml")).replace("skills = [\"*\"]", "skills = [\"greeter\"]");
    std::fs::write(ws.join("tricks.toml"), m + "\n[publish.targets.public.plugins]\n").unwrap();
    commit_all(&ws, "only greeter");
    let r = s.json_in(&ws, &["publish", "public", "--push", "--yes"]);
    assert_eq!(r["suggested_bump"], "major", "{r}");
    let target = checkout(&s, &remote);
    assert!(!target.join("skills/hello").exists());
    assert_eq!(read(&target.join("README.md")), "hand-written readme\n");
}

#[test]
fn licence_gate_blocks_proprietary_vendored_skill_unless_overridden() {
    let (s, _up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//secret-sauce", "--yes"]);
    publish_remote(&s, "acme", "pub", "r\n");
    let m = read(&ws.join("tricks.toml"));
    std::fs::write(ws.join("tricks.toml"), format!("{m}\n[publish.targets.public]\nrepo = \"acme/pub\"\nmarketplace = \"acme-pub\"\n"))
        .unwrap();
    commit_all(&ws, "setup");
    let r = s.json_any_in(&ws, &["publish", "public", "--dry-run"]);
    assert_eq!(r["blocked"], true);
    let lic = r["gates"].as_array().unwrap().iter().find(|g| g["name"] == "licence").unwrap().clone();
    assert_eq!(lic["status"], "fail", "{lic}");
    assert!(lic["details"].to_string().contains("license-override"), "the gate says how to override: {lic}");
    let m = read(&ws.join("tricks.toml")).replace(
        "[skills.secret-sauce]\n",
        "[skills.secret-sauce]\nlicense-override = { justification = \"We hold a separate redistribution agreement with Acme.\" }\n",
    );
    std::fs::write(ws.join("tricks.toml"), m).unwrap();
    commit_all(&ws, "override");
    let r = s.json_in(&ws, &["publish", "public", "--dry-run"]);
    let lic = r["gates"].as_array().unwrap().iter().find(|g| g["name"] == "licence").unwrap().clone();
    assert_eq!(lic["status"], "warn", "{lic}");
}

#[test]
fn contribute_dry_run_contains_only_the_customization() {
    let (s, up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//hello"]);
    let p = ws.join("skills/hello/SKILL.md");
    std::fs::write(&p, read(&p).replace("2. Wave.", "2. Wave.\n3. Smile.")).unwrap();
    commit_all(&ws, "add a step");
    // Upstream moved on meanwhile (non-overlapping).
    let up_md = up.join("skills/hello/SKILL.md");
    std::fs::write(&up_md, read(&up_md).replace("Intro paragraph.", "Intro paragraph, revised.")).unwrap();
    commit_all(&up, "upstream edit");
    let r = s.json_in(&ws, &["contribute", "hello", "--dry-run"]);
    assert_eq!(r["files"].as_array().unwrap().len(), 1, "{r}");
    let wt = PathBuf::from(r["worktree"].as_str().unwrap());
    let content = read(&wt.join("skills/hello/SKILL.md"));
    assert!(content.contains("3. Smile.") && content.contains("revised"), "{content}");
    let diff = git(&wt, &["show", "--stat", "HEAD"]);
    assert!(diff.contains("skills/hello/SKILL.md") && !diff.contains("tricks"), "{diff}");
}

#[test]
fn sync_follows_an_upstream_rename() {
    let (s, up, ws) = setup();
    s.ok_in(&ws, &["vendor", "acme/skills//hello@main"]);
    commit_all(&ws, "vendor hello");
    // Upstream reorganizes: skills/hello → skills/greetings/hello, and edits it.
    std::fs::create_dir_all(up.join("skills/greetings")).unwrap();
    git(&up, &["mv", "skills/hello", "skills/greetings/hello"]);
    let md = up.join("skills/greetings/hello/SKILL.md");
    std::fs::write(&md, read(&md).replace("Keep it short.", "Keep it short. Moved.")).unwrap();
    commit_all(&up, "reorganize");
    set_interval_zero(&s);
    let r = s.json_in(&ws, &["sync"]);
    assert_eq!(r["items"][0]["state"], "merged", "{r}");
    assert!(read(&ws.join("skills/hello/SKILL.md")).contains("Moved."));
    assert!(read(&ws.join("tricks.lock")).contains("upstream_path = \"skills/greetings/hello\""), "{}", read(&ws.join("tricks.lock")));
    commit_all(&ws, "merge");
    // The next check follows the new path.
    let out = s.json_in(&ws, &["outdated"]);
    assert_eq!(out["items"][0]["state"], "up-to-date", "{out}");
    let d = s.json_in(&ws, &["diff", "hello", "base.."]);
    assert!(d.as_array().unwrap().is_empty(), "{d}");
}

#[test]
fn merge_brings_an_experiment_branch_back() {
    let (s, _up, ws) = setup();
    for n in ["greeter", "farewell"] {
        s.ok_in(
            &ws,
            &["create", n, "--description", "Writes greetings for any occasion. Use when the user asks for a greeting card text."],
        );
    }
    commit_all(&ws, "two skills");
    s.ok_in(&ws, &["link", "--agents", "claude"]);
    // An experiment that touches greeter and, by accident, farewell.
    let out = s.json_in(&ws, &["edit", "greeter", "-b", "terse"]);
    let wt = PathBuf::from(out["worktree"].as_str().unwrap());
    let g = wt.join("skills/greeter/SKILL.md");
    std::fs::write(&g, read(&g).replace("## Instructions", "## Instructions (terse)")).unwrap();
    s.ok_in(&ws, &["edit", "greeter", "--commit", "-m", "terse greeter"]);
    let f = wt.join("skills/farewell/SKILL.md");
    std::fs::write(&f, read(&f).replace("## Instructions", "## Instructions (changed too)")).unwrap();
    commit_all(&wt, "farewell too");
    // Meanwhile main moves on elsewhere in greeter: the merge is three-way.
    let mg = ws.join("skills/greeter/SKILL.md");
    std::fs::write(&mg, read(&mg).replace("## Examples", "## Examples (main)")).unwrap();
    commit_all(&ws, "main edit");
    s.ok_in(&ws, &["use", "greeter@terse"]);

    // Default: only the skill's folder, one commit; the other change is reported.
    let r = s.json_in(&ws, &["merge", "greeter@terse"]);
    assert_eq!(r["mode"], "skill", "{r}");
    assert!(r["commit"].is_string(), "{r}");
    assert_eq!(r["other_paths"].as_array().unwrap().len(), 1, "{r}");
    let merged = read(&mg);
    assert!(merged.contains("(terse)") && merged.contains("(main)"), "{merged}");
    assert!(!read(&ws.join("skills/farewell/SKILL.md")).contains("changed too"), "only greeter is merged");
    assert!(git(&ws, &["status", "--porcelain"]).is_empty(), "committed");
    assert_eq!(git(&ws, &["log", "-1", "--format=%s"]), "Merge terse into greeter");
    // The experiment is over: not edited, not used as a variant, links back on main.
    assert!(!read(&ws.join("tricks.toml")).contains("use = \"terse\""));
    assert_eq!(std::fs::read_link(s.home.join(".claude/skills/greeter")).unwrap(), ws.join("skills/greeter"));
    let err = s.fail_in(&ws, &["edit", "greeter", "--done"]);
    assert!(err.contains("not being edited"), "{err}");
    // Nothing left to merge for greeter; --whole-branch takes the rest.
    let err = s.fail_in(&ws, &["merge", "greeter@terse"]);
    assert!(err.contains("already in main"), "{err}");
    let r = s.json_in(&ws, &["merge", "greeter@terse", "--whole-branch"]);
    assert_eq!(r["mode"], "branch", "{r}");
    assert!(read(&ws.join("skills/farewell/SKILL.md")).contains("changed too"));
    assert!(git(&ws, &["log", "-1", "--format=%s"]).starts_with("Merge branch 'terse'"));
    // Diff compares branches of the source repo.
    let d = s.json_in(&ws, &["diff", "farewell", "HEAD~2..head"]).to_string();
    assert!(d.contains("changed too"), "{d}");
    let err = s.fail_in(&ws, &["diff", "farewell", "base..upstream"]);
    assert!(err.contains("tricks outdated"), "{err}");
}
