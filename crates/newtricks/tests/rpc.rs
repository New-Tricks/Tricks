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
    let up = s.upstream(
        "acme",
        "skills",
        &[
            ("skills/hello/SKILL.md", &skill_md("hello", "Say hello. Use when greeting.", "v1\n")),
            (
                "skills/secret/SKILL.md",
                "---\nname: secret\ndescription: Secret recipe. Use when cooking.\nlicense: Proprietary\n---\nbody\n",
            ),
        ],
    );
    git(&up, &["tag", "v1.0.0"]);
    let ws = s.project("my-skills");
    s.ok_in(&ws, &["init"]);
    let proj = s.project("app");
    let mut child = Command::new(env!("CARGO_BIN_EXE_tricks"))
        .args(["serve", "--stdio"])
        .current_dir(&ws)
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
    let mut call = |id: i64, method: &str, params: Value| {
        send(&mut stdin, &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        recv(&mut out)
    };
    let r = call(1, "initialize", json!({}));
    assert!(r["result"]["version"].is_string());
    assert!(r["result"]["source_repo"]["root"].is_string(), "{r}");
    let r = call(2, "show", json!({"skill":"acme/skills//hello"}));
    assert_eq!(r["result"]["canonical"], "github.com/acme/skills//skills/hello@v1.0.0", "{r}");
    // Try an upstream skill in a project.
    let r = call(3, "link", json!({"skill":"acme/skills//hello","to": proj,"agents":["claude"]}));
    assert_eq!(r["result"]["links"][0]["placements"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["result"]["links"][0]["trial"], true);
    // Vendoring a proprietary skill needs confirmation: the error carries the details.
    let r = call(4, "sourceRepo/vendor", json!({"skill":"acme/skills//secret"}));
    assert_eq!(r["error"]["code"], -32001, "{r}");
    assert!(r["error"]["data"]["details"].to_string().contains("Proprietary"), "{r}");
    let r = call(5, "sourceRepo/vendor", json!({"skill":"acme/skills//secret","yes": true}));
    assert_eq!(r["result"]["name"], "secret", "{r}");
    let r = call(6, "sourceRepo/merge", json!({"dryRun": true}));
    assert_eq!(r["result"]["items"][0]["state"], "up-to-date", "{r}");
    let r = call(7, "status", json!({}));
    assert_eq!(r["result"]["source_repo"]["skills"][0]["name"], "secret", "{r}");
    assert_eq!(r["result"]["links"].as_array().unwrap().len(), 1, "{r}");
    let r = call(8, "nope", json!({}));
    assert_eq!(r["error"]["code"], -32000);
    send(&mut stdin, &json!({"jsonrpc":"2.0","method":"exit"}));
    assert!(child.wait().unwrap().success());
}
