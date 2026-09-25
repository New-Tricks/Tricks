//! `tricks serve --stdio`: JSON-RPC 2.0 with LSP-style `Content-Length` framing,
//! used by the VS Code extension. The extension holds no business logic; every
//! method maps onto the same functions as the CLI.

use crate::ctx::{ConfirmationRequired, Ctx, Opts, RpcUi};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const CONFIRMATION_REQUIRED: i64 = -32001;

fn read_message(r: &mut impl BufRead) -> Result<Option<Value>> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let l = line.trim_end();
        if l.is_empty() {
            if len.is_some() {
                break;
            }
            continue;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            len = Some(v.trim().parse().context("bad Content-Length")?);
        }
    }
    let mut buf = vec![0; len.unwrap()];
    r.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

fn write_message(out: &Mutex<std::io::Stdout>, v: &Value) {
    let body = serde_json::to_vec(v).unwrap();
    let mut o = out.lock().unwrap();
    let _ = write!(o, "Content-Length: {}\r\n\r\n", body.len());
    let _ = o.write_all(&body);
    let _ = o.flush();
}

pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let default_cwd: Arc<Mutex<PathBuf>> = Arc::new(Mutex::new(std::env::current_dir()?));
    let offline = Arc::new(Mutex::new(false));
    while let Some(msg) = read_message(&mut reader)? {
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        if method == "exit" {
            break;
        }
        if method == "initialize" {
            if let Some(c) = params.get("cwd").and_then(|c| c.as_str()) {
                *default_cwd.lock().unwrap() = PathBuf::from(c);
            }
            *offline.lock().unwrap() = params.get("offline").and_then(|v| v.as_bool()).unwrap_or(false);
        }
        let (out, cwd, off) = (out.clone(), default_cwd.clone(), offline.clone());
        std::thread::spawn(move || {
            let cwd = params.get("cwd").and_then(|c| c.as_str()).map(PathBuf::from).unwrap_or_else(|| cwd.lock().unwrap().clone());
            let yes = params.get("yes").and_then(|v| v.as_bool()).unwrap_or(false);
            let opts =
                Opts { offline: params.get("offline").and_then(|v| v.as_bool()).unwrap_or(*off.lock().unwrap()), yes, json: true, cwd };
            let result = Ctx::new(opts, Box::new(RpcUi { yes, messages: RefCell::new(Vec::new()) })).and_then(|ctx| {
                let r = dispatch(&ctx, &method, &params);
                let messages = ctx.ui.take_messages();
                r.map(|mut v| {
                    if let Value::Object(m) = &mut v {
                        m.entry("messages").or_insert(json!(messages));
                    } else {
                        v = json!({ "value": v, "messages": messages });
                    }
                    v
                })
            });
            let Some(id) = id else { return };
            let resp = match result {
                Ok(v) => json!({ "jsonrpc": "2.0", "id": id, "result": v }),
                Err(e) => match e.downcast_ref::<ConfirmationRequired>() {
                    Some(c) => {
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": CONFIRMATION_REQUIRED, "message": c.prompt, "data": c } })
                    }
                    None => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": format!("{e:#}") } }),
                },
            };
            write_message(&out, &resp);
        });
    }
    Ok(())
}

fn s<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(|v| v.as_str())
}

fn req<'a>(p: &'a Value, k: &str) -> Result<&'a str> {
    s(p, k).with_context(|| format!("missing parameter `{k}`"))
}

fn b(p: &Value, k: &str) -> bool {
    p.get(k).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn strs(p: &Value, k: &str) -> Vec<String> {
    p.get(k).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn to<T: serde::Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

fn file_payload(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(t) if !bytes.contains(&0) => json!({ "binary": false, "content": t }),
        _ => json!({ "binary": true, "size": bytes.len() }),
    }
}

fn link_options<'a>(p: &'a Value, agents: &'a [String]) -> crate::links::LinkOptions<'a> {
    crate::links::LinkOptions { to: s(p, "to"), global: b(p, "global"), agents, copy: b(p, "copy"), shadow: b(p, "shadow") }
}

pub fn dispatch(ctx: &Ctx, method: &str, p: &Value) -> Result<Value> {
    use crate::source_repo as ws;
    match method {
        "initialize" => {
            let w = ws::current(ctx)?;
            to(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "source_repo": w.map(|w| json!({ "root": w.root, "name": w.name })),
                "agents": crate::agents::AGENTS.iter().map(|a| a.id).collect::<Vec<_>>(),
            }))
        }
        "shutdown" => Ok(json!({})),
        "search" => {
            let a = crate::cli::SearchArgs {
                query: vec![s(p, "query").unwrap_or("").to_string()],
                agent: s(p, "agent").map(String::from),
                trust: s(p, "trust").map(String::from),
                license: s(p, "license").map(String::from),
                source: s(p, "catalog").map(String::from),
                owner: s(p, "owner").map(String::from),
                category: s(p, "category").map(String::from),
                no_scripts: b(p, "noScripts"),
                limit: p.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize,
                refresh: b(p, "refresh"),
                no_live: b(p, "noLive"),
            };
            to(json!({ "results": crate::cli::do_search(ctx, &a)? }))
        }
        "info" => to(crate::inspect::info(ctx, req(p, "skill")?)?),
        "file/read" => {
            let (canonical, bytes) = crate::inspect::read_file(ctx, req(p, "skill")?, req(p, "path")?)?;
            let mut v = file_payload(&bytes);
            v["canonical"] = json!(canonical);
            Ok(v)
        }
        "catalog/list" => to(json!({ "catalogs": crate::catalogs::list(ctx)? })),
        "catalog/add" => {
            if b(p, "recommended") {
                let added = crate::catalogs::add_recommended(ctx)?;
                let rep = crate::catalogs::refresh(ctx, false, None)?;
                return to(json!({ "added": added, "refresh": rep }));
            }
            let (key, kind) = crate::catalogs::add(ctx, req(p, "input")?, s(p, "kind"))?;
            let rep = crate::catalogs::refresh(ctx, true, Some(&key))?;
            to(json!({ "catalog": key, "kind": kind, "refresh": rep }))
        }
        "catalog/remove" => to(json!({ "removed": crate::catalogs::remove(ctx, req(p, "input")?)? })),
        "catalog/refresh" => to(crate::catalogs::refresh(ctx, true, s(p, "catalog"))?),
        "list" => {
            ws::reconcile(ctx)?;
            to(crate::cli_repo::list(ctx)?)
        }
        "link" => to(crate::links::link(ctx, s(p, "skill"), &link_options(p, &strs(p, "agents")))?),
        "try" => to(crate::links::try_skill(ctx, req(p, "skill")?, &link_options(p, &strs(p, "agents")))?),
        "unlink" => to(crate::links::unlink(ctx, s(p, "skill"), &crate::links::UnlinkOptions { to: s(p, "to"), global: b(p, "global"), all: b(p, "all") })?),
        "agents" => to(crate::agents::AGENTS.iter().map(|a| json!({ "id": a.id, "name": a.display, "userDir": ctx.paths.contract(&a.user_path(&ctx.paths.home)), "projectDir": a.project_dir, "mode": if a.follows_links(&ctx.paths.store()) { "link" } else { "copy" } })).collect::<Vec<_>>()),
        "doctor" => to(crate::doctor::run(ctx)?),
        "sourceRepo/init" => to(ws::init(ctx, s(p, "name"), b(p, "agentSkill"))?),
        "sourceRepo/status" => to(ws::status(ctx, &ws::require(ctx)?)?),
        "sourceRepo/create" => to(ws::create(ctx, &ws::require(ctx)?, req(p, "name")?, s(p, "description"), s(p, "from"))?),
        "sourceRepo/vendor" => {
            let o = ws::VendorOptions { name: s(p, "name"), path: s(p, "path"), from: s(p, "from"), base: s(p, "base") };
            to(ws::vendor(ctx, &ws::require(ctx)?, req(p, "skill")?, &o)?)
        }
        "sourceRepo/remove" => to(ws::remove(ctx, &ws::require(ctx)?, req(p, "skill")?)?),
        "sourceRepo/lint" => {
            let w = ws::require(ctx)?;
            let names = strs(p, "skills");
            let mut fixed = Vec::new();
            if b(p, "fix") {
                for (n, sk) in &w.manifest.skills {
                    if names.is_empty() || names.contains(n) {
                        fixed.extend(crate::lint::fix_dir(&w.root.join(&sk.path))?);
                    }
                }
            }
            let mut r = crate::lint::lint_repo(ctx, &w, &names)?;
            r.fixed = fixed;
            let paths: std::collections::BTreeMap<String, String> = w.manifest.skills.iter().map(|(n, sk)| (n.clone(), w.root.join(&sk.path).to_string_lossy().to_string())).collect();
            to(json!({ "report": r, "skillPaths": paths }))
        }
        "sourceRepo/outdated" => to(ws::outdated(ctx, &ws::require(ctx)?, s(p, "skill"))?),
        "sourceRepo/sync" => {
            let o = ws::SyncOptions { only: s(p, "skill"), dry_run: b(p, "dryRun"), cont: b(p, "continue"), abort: b(p, "abort") };
            to(ws::sync(ctx, &ws::require(ctx)?, &o)?)
        }
        "sourceRepo/edit" => {
            let skill = req(p, "skill")?;
            if b(p, "commit") {
                let c = ws::commit_draft(ctx, skill, req(p, "message")?)?;
                if !b(p, "done") {
                    return to(c);
                }
            }
            if b(p, "done") { to(ws::edit_done(ctx, skill)?) } else { to(ws::edit(ctx, skill, s(p, "branch"))?) }
        }
        "sourceRepo/merge" => {
            let o = ws::MergeOptions { whole_branch: b(p, "wholeBranch"), pr: b(p, "pr"), message: s(p, "message") };
            to(ws::merge_branch(ctx, req(p, "spec")?, &o)?)
        }
        "sourceRepo/use" => to(ws::use_variant(ctx, req(p, "spec")?, b(p, "local"), b(p, "reset"))?),
        "sourceRepo/changedFiles" => {
            let w = ws::require(ctx)?;
            to(json!({ "files": ws::changed_files(ctx, &w, req(p, "skill")?, s(p, "from").unwrap_or("head"), s(p, "to").unwrap_or("working"))? }))
        }
        "sourceRepo/versionFile" => {
            let w = ws::require(ctx)?;
            match ws::version_file(ctx, &w, req(p, "skill")?, req(p, "which")?, req(p, "path")?)? {
                Some(bytes) => Ok(file_payload(&bytes)),
                None => Ok(json!({ "missing": true })),
            }
        }
        "sourceRepo/mergeState" => {
            let w = ws::require(ctx)?;
            to(json!({ "state": ws::merge_state(ctx, &w, req(p, "skill")?)?, "skillDir": w.skill_dir(req(p, "skill")?)? }))
        }
        "publish" => to(crate::publish::publish(
            ctx,
            &crate::publish::PublishOptions {
                target: req(p, "target")?.to_string(),
                bump: s(p, "bump").map(String::from),
                dry_run: b(p, "dryRun"),
                push: b(p, "push"),
                pr: b(p, "pr"),
                accept_copyleft: b(p, "acceptCopyleft"),
            },
        )?),
        "contribute" => to(crate::contribute::contribute(ctx, req(p, "skill")?, s(p, "title"), s(p, "body"), b(p, "dryRun"))?),
        other => bail!("unknown method `{other}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_roundtrip() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#;
        let msg = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut r = std::io::BufReader::new(msg.as_bytes());
        let v = read_message(&mut r).unwrap().unwrap();
        assert_eq!(v["method"], "initialize");
        assert!(read_message(&mut r).unwrap().is_none());
    }
}
