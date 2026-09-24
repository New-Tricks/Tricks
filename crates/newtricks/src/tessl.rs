//! Tessl registry adapter (live query). Tessl search results point at skills in GitHub
//! repositories (`sourceUrl` + `path`), so they feed the git index; Tessl's own quality,
//! security and eval scores are kept as listing signals.

use crate::ctx::Ctx;
use crate::github::encode_query;
use crate::id::{SkillId, parse_source_input};
use crate::index;
use anyhow::{Result, bail};
use serde::Deserialize;

pub const CATALOG: &str = "tessl";

fn base_url() -> String {
    std::env::var("TRICKS_TESSL_URL").unwrap_or_else(|_| "https://api.tessl.io".into())
}

#[derive(Deserialize)]
struct Response {
    #[serde(default)]
    data: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    attributes: Attributes,
}

#[derive(Deserialize, Default)]
struct Attributes {
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "sourceUrl", default)]
    source_url: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(rename = "isPrivate", default)]
    is_private: bool,
    #[serde(default)]
    scores: Option<Scores>,
}

#[derive(Deserialize, Default)]
struct Scores {
    #[serde(default)]
    aggregate: Option<f64>,
    #[serde(default)]
    quality: Option<f64>,
    #[serde(rename = "securityLevel", default)]
    security_level: Option<String>,
    #[serde(rename = "evalImprovement", default)]
    eval_improvement: Option<f64>,
    #[serde(default)]
    version: Option<String>,
}

/// A Tessl pointer: repository, skill directory, and signals.
pub struct Pointer {
    pub id: SkillId,
    pub signals: serde_json::Value,
}

fn pointers_from(resp: Response) -> Vec<Pointer> {
    let mut out = Vec::new();
    for it in resp.data {
        let a = it.attributes;
        if it.kind != "skill" || a.is_private {
            continue;
        }
        let (Some(url), Some(path)) = (a.source_url, a.path) else { continue };
        let Ok(src) = parse_source_input(&url) else { continue };
        let dir = path.trim_end_matches("SKILL.md").trim_end_matches("skill.md").trim_matches('/').to_string();
        let s = a.scores.unwrap_or_default();
        let signals = serde_json::json!({
            "quality": s.quality,
            "score": s.aggregate,
            "security": s.security_level,
            "eval_improvement": s.eval_improvement,
            "scored_commit": s.version,
            "name": a.name,
        });
        out.push(Pointer { id: SkillId::new(src, &dir), signals });
    }
    out
}

/// Live search: index the pointed-to skills and attach Tessl signals.
pub fn live_search(ctx: &Ctx, q: &str, max: usize) -> Result<usize> {
    if ctx.opts.offline || q.trim().is_empty() || crate::sources::live_fresh(ctx, CATALOG, q)? {
        return Ok(0);
    }
    let url = format!("{}/experimental/search?q={}&page%5Bsize%5D={max}", base_url(), encode_query(q));
    let r = ctx.gh.get_public(&url)?;
    if r.status != 200 {
        bail!("Tessl search returned {}", r.status);
    }
    let resp: Response = serde_json::from_slice(&r.body)?;
    let pointers = pointers_from(resp);
    let jobs: Vec<(crate::id::SourceId, String)> = pointers.iter().map(|p| (p.id.source.clone(), p.id.path.clone())).collect();
    crate::sources::ensure_pointers_indexed(ctx, &jobs);
    let mut n = 0;
    for p in pointers {
        let id = p.id.to_string();
        if index::skill_exists(ctx, &id)? {
            index::add_listing_signals(ctx, &id, CATALOG, None, &p.signals)?;
            n += 1;
        }
    }
    crate::sources::live_mark(ctx, CATALOG, q)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pointers() {
        let body = r#"{"data":[
          {"type":"skill","attributes":{"name":"pdf","sourceUrl":"https://github.com/Acme/Tools","path":"skills/pdf/SKILL.md","isPrivate":false,
            "scores":{"aggregate":0.7,"quality":0.9,"securityLevel":"HIGH","evalImprovement":null,"version":"abc"}}},
          {"type":"tile","attributes":{"name":"old"}},
          {"type":"skill","attributes":{"name":"private","sourceUrl":"https://github.com/a/b","path":"x/SKILL.md","isPrivate":true}},
          {"type":"skill","attributes":{"name":"root","sourceUrl":"https://github.com/a/root-skill","path":"SKILL.md"}}
        ]}"#;
        let p = pointers_from(serde_json::from_str(body).unwrap());
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].id.to_string(), "github.com/acme/tools//skills/pdf");
        assert_eq!(p[0].signals["security"], "HIGH");
        assert_eq!(p[1].id.to_string(), "github.com/a/root-skill//.");
    }
}
