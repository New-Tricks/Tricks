//! `tricks self-update` for standalone installs (spec §16). Homebrew and
//! VS Code-bundled binaries are updated by their package managers.

use crate::ctx::Ctx;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Release repository; override with TRICKS_RELEASE_REPO.
pub const RELEASE_REPO: &str = "new-tricks/tricks";

#[derive(Debug, Serialize)]
pub struct SelfUpdateReport {
    pub current: String,
    pub latest: Option<String>,
    pub updated: bool,
    pub message: String,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn target_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        ("windows", "aarch64") => "aarch64-pc-windows-msvc",
        _ => "unknown",
    }
}

pub fn run(ctx: &Ctx, check_only: bool) -> Result<SelfUpdateReport> {
    let current = env!("CARGO_PKG_VERSION").to_string();
    let exe = std::env::current_exe()?;
    let exe_s = exe.to_string_lossy();
    if exe_s.contains("/Cellar/") || exe_s.contains("/homebrew/") || exe_s.contains("/linuxbrew/") {
        return Ok(SelfUpdateReport {
            current,
            latest: None,
            updated: false,
            message: "installed with Homebrew: run `brew upgrade tricks`".into(),
        });
    }
    let in_extension = (exe_s.contains("/extensions/") || exe_s.contains("\\extensions\\"))
        && (exe_s.contains("new-tricks") || exe_s.contains("newtricks"));
    if in_extension {
        return Ok(SelfUpdateReport {
            current,
            latest: None,
            updated: false,
            message: "bundled with the VS Code extension: it updates with the extension".into(),
        });
    }
    let repo = std::env::var("TRICKS_RELEASE_REPO").unwrap_or_else(|_| RELEASE_REPO.into());
    let rel: Release = ctx.gh.api_json("github.com", &format!("/repos/{repo}/releases/latest")).context("checking for releases")?;
    let latest = rel.tag_name.trim_start_matches('v').to_string();
    let newer = match (semver::Version::parse(&latest), semver::Version::parse(&current)) {
        (Ok(l), Ok(c)) => l > c,
        _ => latest != current,
    };
    if !newer {
        return Ok(SelfUpdateReport {
            current: current.clone(),
            latest: Some(latest),
            updated: false,
            message: format!("New Tricks {current} is up to date"),
        });
    }
    if check_only {
        return Ok(SelfUpdateReport {
            current: current.clone(),
            latest: Some(latest.clone()),
            updated: false,
            message: format!("New Tricks {latest} is available (you have {current})"),
        });
    }
    let triple = target_triple();
    let asset_name = format!("tricks-{triple}.tar.gz");
    let asset =
        rel.assets.iter().find(|a| a.name == asset_name).with_context(|| format!("release {} has no asset {asset_name}", rel.tag_name))?;
    let sum = rel.assets.iter().find(|a| a.name == format!("{asset_name}.sha256")).context("release has no checksum file")?;
    let tarball = ctx.gh.get_public(&asset.browser_download_url)?;
    let sums = ctx.gh.get_public(&sum.browser_download_url)?;
    if tarball.status != 200 || sums.status != 200 {
        bail!("download failed");
    }
    use sha2::Digest;
    let actual = hex::encode(sha2::Sha256::digest(&tarball.body));
    let expected = String::from_utf8_lossy(&sums.body).split_whitespace().next().unwrap_or("").to_string();
    if actual != expected {
        bail!("checksum mismatch for {asset_name}");
    }
    let tmp = tempfile::tempdir()?;
    let archive = tmp.path().join(&asset_name);
    std::fs::write(&archive, &tarball.body)?;
    // System tar handles gzip on macOS, Linux and Windows 10+ (bsdtar).
    let st = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(tmp.path())
        .status()
        .context("tar is required for self-update")?;
    if !st.success() {
        bail!("could not extract {asset_name}");
    }
    let bin = tmp.path().join(if cfg!(windows) { "tricks.exe" } else { "tricks" });
    let staged = exe.with_extension("new");
    std::fs::copy(&bin, &staged)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    let old = exe.with_extension("old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&exe, &old)?;
    std::fs::rename(&staged, &exe)?;
    let _ = std::fs::remove_file(&old);
    Ok(SelfUpdateReport { current, latest: Some(latest.clone()), updated: true, message: format!("updated to New Tricks {latest}") })
}
