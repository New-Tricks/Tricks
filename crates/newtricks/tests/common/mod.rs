//! Hermetic test sandbox: isolated home/config/data directories and local "GitHub"
//! repositories served through TRICKS_HOST_MAP.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

pub struct Sandbox {
    pub dir: tempfile::TempDir,
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub fixtures: PathBuf,
    pub identity: Option<String>,
    /// Extra environment for every command (applied last).
    pub env: Vec<(String, String)>,
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
            env: Vec::new(),
        };
        for d in [&s.home, &s.config, &s.data, &s.fixtures] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(s.config.join("tricks.toml"), "[settings]\ndefault_catalogs = false\nagents = [\"claude\", \"codex\"]\n").unwrap();
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
            .env("TRICKS_TESSL_URL", "http://127.0.0.1:9")
            .env("TRICKS_CLAWHUB_URL", "http://127.0.0.1:9")
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
        for (k, v) in &self.env {
            c.env(k, v);
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

pub fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("reading {}: {e}", p.display()))
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

/// Paths (including the query string) → response bodies, mutable while serving.
pub type Files = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// Minimal HTTP/1.1 server answering GETs from `files` (404 otherwise); returns its base URL.
pub fn serve(files: Files) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            let files = files.clone();
            std::thread::spawn(move || {
                let mut s = stream;
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).is_err() || h.trim().is_empty() {
                        break;
                    }
                }
                let body = files.lock().unwrap().get(&path).cloned();
                let _ = match body {
                    Some(b) => {
                        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                        s.write_all(&b)
                    }
                    None => write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                };
            });
        }
    });
    format!("http://{addr}")
}

pub fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        for (p, c) in files {
            z.start_file(*p, zip::write::SimpleFileOptions::default()).unwrap();
            z.write_all(c).unwrap();
        }
        z.finish().unwrap();
    }
    buf.into_inner()
}
