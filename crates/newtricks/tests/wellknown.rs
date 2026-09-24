//! `.well-known/agent-skills` discovery end to end against a local HTTP server.
// Asserts on symlinked placements; Windows deploys copies (spec §8).
#![cfg(unix)]

mod common;
use common::*;
use sha2::Digest;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// (name, type, url, body, digest override)
type IndexEntry<'a> = (&'a str, &'a str, &'a str, &'a [u8], Option<String>);

fn digest(b: &[u8]) -> String {
    format!("sha256:{}", hex::encode(sha2::Sha256::digest(b)))
}

fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default()));
    for (p, c) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(c.len() as u64);
        h.set_mode(if p.ends_with(".sh") { 0o755 } else { 0o644 });
        h.as_gnu_mut().unwrap().name[..p.len()].copy_from_slice(p.as_bytes());
        h.set_cksum();
        b.append(&h, *c).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

fn md(name: &str, v: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: Greets in {v}. Use when the user asks for a greeting.\nlicense: MIT\n---\n# {name} {v}\n")
        .into_bytes()
}

fn publish_index(files: &Files, entries: &[IndexEntry]) {
    let mut f = files.lock().unwrap();
    let mut skills = Vec::new();
    for (name, kind, url, body, dig) in entries {
        // Serve relative URLs where RFC 3986 resolution against index.json puts them.
        let served = if url.starts_with('/') { url.to_string() } else { format!("/.well-known/agent-skills/{url}") };
        f.insert(served, body.to_vec());
        skills.push(serde_json::json!({
            "name": name, "type": kind, "description": format!("{name} skill"),
            "url": url, "digest": dig.clone().unwrap_or_else(|| digest(body)),
        }));
    }
    let idx = serde_json::json!({ "$schema": "https://schemas.agentskills.io/discovery/0.2.0/schema.json", "skills": skills });
    f.insert("/.well-known/agent-skills/index.json".into(), serde_json::to_vec(&idx).unwrap());
}

#[test]
fn wellknown_skill_md_and_archives() {
    let s = Sandbox::new();
    let files: Files = Arc::new(Mutex::new(HashMap::new()));
    let base = serve(files.clone());
    let single = md("single", "v1");
    let tgz = tar_gz(&[("SKILL.md", &md("tarred", "v1")), ("scripts/run.sh", b"#!/bin/sh\necho hi\n")]);
    let zipped = zip(&[("SKILL.md", &md("zipped", "v1")), ("references/notes.md", b"notes\n")]);
    let evil = tar_gz(&[("SKILL.md", &md("evil", "v1")), ("../../escape.txt", b"gotcha")]);
    publish_index(
        &files,
        &[
            ("single", "skill-md", "/.well-known/agent-skills/single/SKILL.md", &single, None),
            ("tarred", "archive", "tarred.tar.gz", &tgz, None), // relative to index.json
            ("zipped", "archive", "/.well-known/agent-skills/zipped.zip", &zipped, None),
            ("tampered", "skill-md", "/t.md", &md("tampered", "v1"), Some(digest(b"something else"))),
            ("evil", "archive", "/evil.tar.gz", &evil, None),
        ],
    );
    let out = s.cmd(&s.root(), &["source", "add", &base, "--kind", "wellknown"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "{stderr}");
    assert!(stderr.contains("tampered") && stderr.contains("digest mismatch"), "{stderr}");
    assert!(stderr.contains("evil") && stderr.contains("escapes"), "{stderr}");

    let r = s.json(&["search", "--no-live", "greeting"]);
    let names: Vec<&str> = r.as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    for n in ["single", "tarred", "zipped"] {
        assert!(names.contains(&n), "{n} missing from {names:?}");
    }
    assert!(!names.contains(&"tampered") && !names.contains(&"evil"));

    let host = base.trim_start_matches("http://");
    s.ok(&["add", &format!("{host}/.well-known/agent-skills//tarred"), "--agents", "claude"]);
    s.ok(&["add", &format!("{host}/.well-known/agent-skills//zipped"), "--agents", "claude"]);
    s.ok(&["add", &format!("{host}/.well-known/agent-skills//single"), "--agents", "claude"]);
    let skills = s.home.join(".claude/skills");
    assert!(skills.join("tarred/scripts/run.sh").exists());
    assert!(skills.join("zipped/references/notes.md").exists());
    assert!(std::fs::read_to_string(skills.join("single/SKILL.md")).unwrap().contains("single v1"));

    // A new version is detected by digest and applied on update.
    let tgz2 = tar_gz(&[("SKILL.md", &md("tarred", "v2")), ("scripts/run.sh", b"#!/bin/sh\necho hi\n")]);
    publish_index(
        &files,
        &[
            ("single", "skill-md", "/.well-known/agent-skills/single/SKILL.md", &single, None),
            ("tarred", "archive", "tarred.tar.gz", &tgz2, None),
            ("zipped", "archive", "/.well-known/agent-skills/zipped.zip", &zipped, None),
        ],
    );
    let cfg = s.config.join("tricks.toml");
    std::fs::write(&cfg, std::fs::read_to_string(&cfg).unwrap().replace("[settings]", "[settings]\nfetch_interval = \"0s\"")).unwrap();
    let o = s.json(&["outdated"]);
    let pending: Vec<&str> = o.as_array().unwrap().iter().map(|x| x["name"].as_str().unwrap()).collect();
    assert_eq!(pending, vec!["tarred"], "{o}");
    s.ok(&["update", "--yes"]);
    assert!(std::fs::read_to_string(skills.join("tarred/SKILL.md")).unwrap().contains("tarred v2"));
    assert!(s.json(&["outdated"]).as_array().unwrap().is_empty());
}

#[test]
fn rejects_unknown_schema() {
    let s = Sandbox::new();
    let files: Files = Arc::new(Mutex::new(HashMap::new()));
    let base = serve(files.clone());
    files.lock().unwrap().insert(
        "/.well-known/agent-skills/index.json".into(),
        br#"{"$schema":"https://schemas.agentskills.io/discovery/9.9.9/schema.json","skills":[]}"#.to_vec(),
    );
    let out = s.cmd(&s.root(), &["source", "add", &base, "--kind", "wellknown"]);
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unrecognized $schema"), "{err}");
}
