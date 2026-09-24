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

pub fn dispatch(ctx: &Ctx, method: &str, p: &Value) -> Result<Value> {
    use crate::workspace as ws;
    match method {
        "initialize" => {
            let w = ws::current(ctx)?;
            to(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "workspace": w.map(|w| json!({ "root": w.root, "name": w.name })),
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
                source: s(p, "source").map(String::from),
                owner: s(p, "owner").map(String::from),
                category: s(p, "category").map(String::from),
                installed: b(p, "installed"),
                no_scripts: b(p, "noScripts"),
                limit: p.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize,
                refresh: b(p, "refresh"),
                no_live: b(p, "noLive"),
            };
            to(json!({ "results": crate::cli::do_search(ctx, &a)? }))
        }
        "show" => to(crate::inspect::show(ctx, req(p, "skill")?)?),
        "file/read" => {
            let (canonical, bytes) = crate::inspect::read_file(ctx, req(p, "skill")?, req(p, "path")?)?;
            let mut v = file_payload(&bytes);
            v["canonical"] = json!(canonical);
            Ok(v)
        }
        "sources/list" => to(json!({ "sources": crate::sources::list(ctx)? })),
        "sources/add" => {
            let (key, kind) = crate::sources::add(ctx, req(p, "input")?, s(p, "kind"))?;
            let rep = crate::sources::refresh(ctx, true, Some(&key))?;
            to(json!({ "source": key, "kind": kind, "refresh": rep }))
        }
        "sources/remove" => to(json!({ "removed": crate::sources::remove(ctx, req(p, "input")?)? })),
        "sources/refresh" => to(crate::sources::refresh(ctx, true, s(p, "source"))?),
        "workbench/add" => {
            let policy = s(p, "update").map(crate::config::Policy::parse).transpose()?;
            to(crate::workbench::add(ctx, req(p, "skill")?, &strs(p, "agents"), policy, b(p, "copy"), b(p, "shadow"))?)
        }
        "workbench/remove" => to(json!({ "removed": crate::workbench::remove(ctx, req(p, "skill")?)? })),
        "workbench/install" => to(crate::workbench::install(ctx, b(p, "frozen"))?),
        "workbench/update" => to(json!({ "updates": crate::workbench::update(ctx, s(p, "skill"))? })),
        "workbench/outdated" => to(json!({ "pending": crate::workbench::outdated(ctx)? })),
        "workbench/rollback" => to(crate::workbench::rollback(ctx, req(p, "skill")?)?),
        "workbench/policy" => {
            let pol = crate::config::Policy::parse(req(p, "policy")?)?;
            to(json!({ "id": crate::workbench::set_policy(ctx, req(p, "skill")?, pol)? }))
        }
        "status" => {
            let wb = crate::workbench::status(ctx)?;
            let w = match ws::current(ctx)? {
                Some(w) => Some(ws::status(ctx, &w)?),
                None => None,
            };
            to(json!({ "workbench": wb, "workspace": w }))
        }
        "link" => to(crate::links::link(ctx, req(p, "skill")?, s(p, "to"), b(p, "global"), &strs(p, "agents"), b(p, "copy"), b(p, "shadow"))?),
        "unlink" => to(crate::links::unlink(ctx, s(p, "skill"), s(p, "to"), b(p, "global"), b(p, "all"))?),
        "agents" => to(crate::agents::AGENTS.iter().map(|a| json!({ "id": a.id, "name": a.display, "userDir": ctx.paths.contract(&a.user_path(&ctx.paths.home)), "projectDir": a.project_dir, "mode": if a.follows_links(&ctx.paths.store()) { "link" } else { "copy" } })).collect::<Vec<_>>()),
        "doctor" => to(crate::doctor::run(ctx)?),
        "agentSkill/status" => to(json!({ "status": crate::agentskill::status(ctx)?, "offer": crate::agentskill::should_offer(ctx)? })),
        "agentSkill/install" => to(json!({ "paths": crate::agentskill::install(ctx, &strs(p, "agents"))? })),
        "agentSkill/remove" => to(json!({ "paths": crate::agentskill::remove(ctx)? })),
        "agentSkill/dismiss" => {
            crate::agentskill::mark_offered(ctx)?;
            Ok(json!({ "ok": true }))
        }
        "workspace/init" => to(ws::init(ctx, s(p, "name"), b(p, "agentSkill"))?),
        "workspace/status" => to(ws::status(ctx, &ws::require(ctx)?)?),
        "workspace/vendor" => to(ws::vendor(ctx, &ws::require(ctx)?, req(p, "skill")?, s(p, "name"), s(p, "path"))?),
        "workspace/import" => to(ws::import(ctx, &ws::require(ctx)?, req(p, "folder")?, s(p, "name"), s(p, "upstream"), s(p, "base"))?),
        "workspace/new" => to(ws::new_skill(&ws::require(ctx)?, req(p, "name")?, s(p, "description"))?),
        "workspace/lint" => {
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
            let mut r = crate::lint::lint_workspace(ctx, &w, &names)?;
            r.fixed = fixed;
            let paths: std::collections::BTreeMap<String, String> = w.manifest.skills.iter().map(|(n, sk)| (n.clone(), w.root.join(&sk.path).to_string_lossy().to_string())).collect();
            to(json!({ "report": r, "skillPaths": paths }))
        }
        "workspace/update" => to(ws::update(ctx, &ws::require(ctx)?, s(p, "skill"), b(p, "continue"), b(p, "abort"))?),
        "workspace/outdated" => to(ws::outdated(ctx, &ws::require(ctx)?)?),
        "workspace/install" => to(ws::install_dev(ctx, &ws::require(ctx)?, &strs(p, "agents"))?),
        "workspace/edit" => to(ws::edit(ctx, req(p, "skill")?, s(p, "branch"))?),
        "workspace/commit" => to(ws::commit(ctx, req(p, "skill")?, req(p, "message")?)?),
        "workspace/use" => to(ws::use_variant(ctx, req(p, "spec")?, b(p, "local"), b(p, "reset"))?),
        "workspace/changedFiles" => {
            let w = ws::require(ctx)?;
            to(json!({ "files": ws::changed_files(ctx, &w, req(p, "skill")?, s(p, "from").unwrap_or("base"), s(p, "to").unwrap_or("working"))? }))
        }
        "workspace/versionFile" => {
            let w = ws::require(ctx)?;
            match ws::version_file(ctx, &w, req(p, "skill")?, req(p, "which")?, req(p, "path")?)? {
                Some(bytes) => Ok(file_payload(&bytes)),
                None => Ok(json!({ "missing": true })),
            }
        }
        "workspace/mergeState" => {
            let w = ws::require(ctx)?;
            to(json!({ "state": ws::merge_state(ctx, &w, req(p, "skill")?)?, "skillDir": w.skill_dir(req(p, "skill")?)? }))
        }
        "workspace/allowLicense" => {
            let w = ws::require(ctx)?;
            ws::set_license_override(&w, req(p, "skill")?, req(p, "justification")?)?;
            Ok(json!({ "ok": true }))
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
        "pr" => to(crate::pr::pr(ctx, req(p, "skill")?, s(p, "title"), s(p, "body"), b(p, "dryRun"))?),
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
