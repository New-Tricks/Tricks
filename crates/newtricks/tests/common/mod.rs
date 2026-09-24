//! Hermetic test sandbox: isolated home/config/data directories and local "GitHub"
//! repositories served through TRICKS_HOST_MAP.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub struct Sandbox {
    pub dir: tempfile::TempDir,
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub fixtures: PathBuf,
    pub identity: Option<String>,
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(o.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

pub fn skill_md(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\nlicense: MIT\n---\n{body}")
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let root = newtricks::paths::canon(dir.path()).unwrap();
        let s = Sandbox {
            home: root.join("home"),
            config: root.join("config"),
            data: root.join("data"),
            fixtures: root.join("fixtures"),
            dir,
            identity: None,
        };
        for d in [&s.home, &s.config, &s.data, &s.fixtures] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(s.config.join("tricks.toml"), "[settings]\ndefault_sources = false\nagents = [\"claude\", \"codex\"]\n").unwrap();
        s
    }

    pub fn root(&self) -> PathBuf {
        newtricks::paths::canon(self.dir.path()).unwrap()
    }

    pub fn cmd(&self, cwd: &Path, args: &[&str]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_tricks"));
        c.args(args)
            .current_dir(cwd)
            .env("TRICKS_HOME", &self.home)
            .env("TRICKS_CONFIG_DIR", &self.config)
            .env("TRICKS_DATA_DIR", &self.data)
            .env("TRICKS_HOST_MAP", format!("github.com={}", self.fixtures.display()))
            .env("TRICKS_NO_GH", "1")
            .env("TRICKS_NO_API", "1")
            .env("TRICKS_SKILLS_SH_URL", "http://127.0.0.1:9")
            .env_remove("GITHUB_TOKEN")
            .env_remove("TRICKS_GITHUB_TOKEN")
            .env_remove("TRICKS_LINK_MODE")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com");
        match &self.identity {
            Some(i) => {
                c.env("TRICKS_IDENTITY", i);
            }
            None => {
                c.env_remove("TRICKS_IDENTITY");
            }
        }
        c.output().unwrap()
    }

    /// Run in `cwd`; assert success; return stdout.
    pub fn ok_in(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self.cmd(cwd, args);
        assert!(
            o.status.success(),
            "tricks {:?} failed\nstdout: {}\nstderr: {}",
            args,
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    pub fn ok(&self, args: &[&str]) -> String {
        self.ok_in(&self.root(), args)
    }

    pub fn fail_in(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self.cmd(cwd, args);
        assert!(!o.status.success(), "tricks {:?} unexpectedly succeeded: {}", args, String::from_utf8_lossy(&o.stdout));
        format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
    }

    pub fn fail(&self, args: &[&str]) -> String {
        self.fail_in(&self.root(), args)
    }

    pub fn json_in(&self, cwd: &Path, args: &[&str]) -> serde_json::Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let out = self.ok_in(cwd, &a);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("bad json from {args:?}: {e}\n{out}"))
    }

    /// JSON output regardless of exit status (e.g. a blocked publish exits 1).
    pub fn json_any_in(&self, cwd: &Path, args: &[&str]) -> serde_json::Value {
        let mut a = vec!["--json"];
        a.extend_from_slice(args);
        let o = self.cmd(cwd, &a);
        let out = String::from_utf8_lossy(&o.stdout).to_string();
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("bad json from {args:?}: {e}\n{out}\n{}", String::from_utf8_lossy(&o.stderr)))
    }

    pub fn json(&self, args: &[&str]) -> serde_json::Value {
        self.json_in(&self.root(), args)
    }

    /// Create an upstream repository `github.com/<owner>/<name>` with the given files.
    pub fn upstream(&self, owner: &str, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let d = self.fixtures.join(owner).join(name);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q", "-b", "main"]);
        for (p, c) in files {
            write(&d.join(p), c);
        }
        git(&d, &["add", "-A"]);
        git(&d, &["commit", "-qm", "initial"]);
        d
    }

    /// A plain project repository to link skills into.
    pub fn project(&self, name: &str) -> PathBuf {
        let d = self.root().join("projects").join(name);
        std::fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q", "-b", "main"]);
        write(&d.join("README.md"), "project\n");
        git(&d, &["add", "-A"]);
        git(&d, &["commit", "-qm", "init"]);
        d
    }
}

pub fn write(p: &Path, content: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

pub fn commit_all(repo: &Path, msg: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", msg]);
    git(repo, &["rev-parse", "HEAD"])
}
