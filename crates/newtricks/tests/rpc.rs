//! `serve --stdio` protocol test: framing, dispatch, confirmation errors.

mod common;
use common::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn send(stdin: &mut impl Write, v: &Value) {
    let body = serde_json::to_vec(v).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn recv(r: &mut impl BufRead) -> Value {
    let mut len = 0;
    loop {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            len = v.trim().parse().unwrap();
        }
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf).unwrap();
    serde_json::from_slice(&buf).unwrap()
}

#[test]
fn serve_stdio_roundtrip() {
    let s = Sandbox::new();
    let up = s.upstream("acme", "skills", &[("skills/hello/SKILL.md", &skill_md("hello", "Say hello. Use when greeting.", "v1\n"))]);
    git(&up, &["tag", "v1.0.0"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_tricks"))
        .args(["serve", "--stdio"])
        .current_dir(s.root())
        .env("TRICKS_HOME", &s.home)
        .env("TRICKS_CONFIG_DIR", &s.config)
        .env("TRICKS_DATA_DIR", &s.data)
        .env("TRICKS_HOST_MAP", format!("github.com={}", s.fixtures.display()))
        .env("TRICKS_NO_GH", "1")
        .env("TRICKS_NO_API", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    send(&mut stdin, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"cwd": s.root()}}));
    let r = recv(&mut out);
    assert_eq!(r["id"], 1);
    assert!(r["result"]["version"].is_string());
    send(&mut stdin, &json!({"jsonrpc":"2.0","id":2,"method":"show","params":{"skill":"acme/skills//hello"}}));
    let r = recv(&mut out);
    assert_eq!(r["result"]["canonical"], "github.com/acme/skills//skills/hello@v1.0.0", "{r}");
    send(&mut stdin, &json!({"jsonrpc":"2.0","id":3,"method":"user/add","params":{"skill":"acme/skills//hello","agents":["claude"]}}));
    let r = recv(&mut out);
    assert!(r["result"]["placements"].as_array().unwrap().len() == 1, "{r}");
    // Update with a pending change and no `yes` → confirmation_required error.
    std::fs::write(up.join("skills/hello/SKILL.md"), skill_md("hello", "Say hello. Use when greeting.", "v2\n")).unwrap();
    commit_all(&up, "v2");
    git(&up, &["tag", "v1.1.0"]);
    let cfg = s.config.join("tricks.toml");
    std::fs::write(&cfg, std::fs::read_to_string(&cfg).unwrap().replace("[settings]", "[settings]\nfetch_interval = \"0s\"")).unwrap();
    send(&mut stdin, &json!({"jsonrpc":"2.0","id":4,"method":"user/update","params":{}}));
    let r = recv(&mut out);
    assert_eq!(r["error"]["code"], -32001, "{r}");
    assert!(r["error"]["data"]["details"].to_string().contains("v1.1.0"));
    send(&mut stdin, &json!({"jsonrpc":"2.0","id":5,"method":"user/update","params":{"yes": true}}));
    let r = recv(&mut out);
    assert_eq!(r["result"]["updates"][0]["applied"], true, "{r}");
    send(&mut stdin, &json!({"jsonrpc":"2.0","id":6,"method":"nope"}));
    let r = recv(&mut out);
    assert_eq!(r["error"]["code"], -32000);
    send(&mut stdin, &json!({"jsonrpc":"2.0","method":"exit"}));
    assert!(child.wait().unwrap().success());
}
