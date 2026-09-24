//! `tricks pr <skill>` (spec §11): contribute a customization back upstream. Only
//! the B → C change of that one skill leaves the workspace.

use crate::ctx::Ctx;
use crate::git::{self, git};
use crate::id::SkillId;
use crate::merge;
use crate::resolve::{Fetch, open_mirror};
use crate::skill;
use crate::store;
use crate::workspace;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Serialize)]
pub struct PrReport {
    pub skill: String,
    pub upstream: String,
    pub branch: String,
    pub files: Vec<String>,
    pub diffstat: String,
    pub worktree: String,
    pub fork: Option<String>,
    pub url: Option<String>,
    pub dry_run: bool,
}

pub fn pr(ctx: &Ctx, name: &str, title: Option<&str>, body: Option<&str>, dry_run: bool) -> Result<PrReport> {
    let ws = workspace::require(ctx)?;
    let s = ws.skill(name)?.clone();
    let upstream = s.upstream.clone().context("this skill has no upstream (local original); nothing to contribute back")?;
    let locked = ws.lock.skills.get(name).cloned().unwrap_or_default();
    let base = locked.base.clone().context("no base commit recorded")?;
    let base_id = SkillId::parse_canonical(&upstream)?;
    let base_path = base_id.path.clone();
    let up_path = locked.upstream_path.clone().unwrap_or_else(|| base_path.clone());
    if !base_id.source.is_github() {
        bail!("pull requests are supported for GitHub upstreams only");
    }
    let mirror = open_mirror(ctx, &base_id.source, Fetch::Always)?;
    let refs = crate::resolve::mirror_refs(&mirror)?;
    let default = refs.default_branch.clone().context("upstream has no default branch")?;
    let u_commit = refs.heads.get(&default).cloned().context("no default branch commit")?;
    mirror.ensure_commit(&base)?;
    let b_tree = mirror.tree_at(&base, &base_path).context("base revision lacks the skill")?;
    let b_dir = store::from_mirror(ctx, &mirror, &base, &base_path, &b_tree)?;

    // C without tricks-only metadata and merge sidecars.
    let c_tmp = tempfile::tempdir()?;
    store::copy_dir(&ws.skill_dir(name)?, c_tmp.path())?;
    let skill_md = c_tmp.path().join("SKILL.md");
    if let Ok(t) = std::fs::read_to_string(&skill_md) {
        std::fs::write(&skill_md, skill::strip_metadata_prefix(&t, "tricks-"))?;
    }
    for e in walkdir::WalkDir::new(c_tmp.path()).into_iter().flatten() {
        if e.path().to_string_lossy().ends_with(merge::UPSTREAM_SUFFIX) {
            let _ = std::fs::remove_file(e.path());
        }
    }
    if crate::treehash::tree_hash(c_tmp.path())?.as_deref() == Some(b_tree.as_str()) {
        bail!("`{name}` has no changes relative to its upstream base");
    }

    // Worktree of upstream at its current default branch; replay B → C onto it.
    let wt_root = ctx.paths.work().join("pr").join(format!("{}-{name}-{}", ws.key(), crate::state::now()));
    std::fs::create_dir_all(wt_root.parent().unwrap())?;
    let branch = format!("tricks/{name}-{}", crate::state::now());
    git(&mirror.dir, &["worktree", "add", "-q", "-b", &branch, &wt_root.to_string_lossy(), &u_commit])?;
    let target_dir = wt_root.join(&up_path);
    std::fs::create_dir_all(&target_dir)?;
    let outcome = merge::three_way(&b_dir, &target_dir, c_tmp.path(), ("upstream", "base", "yours"))?;
    if !outcome.is_clean() {
        let _ = git(&mirror.dir, &["worktree", "remove", "--force", &wt_root.to_string_lossy()]);
        bail!(
            "your change conflicts with the current upstream ({}); run `tricks update {name}` first",
            outcome.conflicts.iter().map(|c| c.path.clone()).collect::<Vec<_>>().join(", ")
        );
    }
    git(&wt_root, &["add", "-A", "--", &up_path])?;
    let files: Vec<String> = git(&wt_root, &["diff", "--cached", "--name-status"])?.lines().map(String::from).collect();
    let diffstat = git(&wt_root, &["diff", "--cached", "--stat"])?;
    let msg = title.map(String::from).unwrap_or_else(|| format!("Improve {name} skill"));
    git(&wt_root, &["-c", "commit.gpgsign=false", "commit", "-q", "-m", &msg])?;

    let mut rep = PrReport {
        skill: name.into(),
        upstream: base_id.source.to_string(),
        branch: branch.clone(),
        files: files.clone(),
        diffstat: diffstat.clone(),
        worktree: wt_root.to_string_lossy().to_string(),
        fork: None,
        url: None,
        dry_run,
    };
    if dry_run {
        return Ok(rep);
    }
    let mut details =
        vec![format!("upstream: {}", base_id.source), "these changes will become public in a fork and a pull request:".into()];
    details.extend(files.iter().map(|f| format!("  {f}")));
    if !ctx.confirm(&format!("Open a pull request to {}?", base_id.source), &details)? {
        bail!("cancelled (the prepared branch is in {})", wt_root.display());
    }
    let (login, _) = ctx.gh.identity(&base_id.source.host).context("sign in with `gh auth login` to open pull requests")?;
    let full = base_id.source.full_name().to_string();
    let fork =
        Command::new("gh").args(["repo", "fork", &full, "--clone=false", "--remote=false"]).output().context("running gh repo fork")?;
    if !fork.status.success() {
        let err = String::from_utf8_lossy(&fork.stderr);
        if !err.contains("already exists") {
            bail!("gh repo fork failed: {err}");
        }
    }
    let fork_url = format!("https://{}/{login}/{}.git", base_id.source.host, base_id.source.name());
    rep.fork = Some(fork_url.clone());
    git(&wt_root, &["push", "-q", &fork_url, &format!("HEAD:refs/heads/{branch}")])?;
    let body =
        body.map(String::from).unwrap_or_else(|| format!("Changes to `{up_path}` contributed via New Tricks.\n\n```\n{diffstat}\n```"));
    let out = Command::new("gh")
        .args([
            "pr",
            "create",
            "--repo",
            &full,
            "--head",
            &format!("{login}:{branch}"),
            "--base",
            &default,
            "--title",
            &msg,
            "--body",
            &body,
        ])
        .output()
        .context("running gh pr create")?;
    if !out.status.success() {
        bail!("gh pr create failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    rep.url = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
    let _ = git(&mirror.dir, &["worktree", "remove", "--force", &wt_root.to_string_lossy()]);
    let _ = git::git_ok(&mirror.dir, &["worktree", "prune"]);
    Ok(rep)
}
