//! GitHub access with borrowed credentials (spec §13): env → `gh auth token` → the
//! token VS Code passes to `serve` → anonymous. Nothing is ever stored.

use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;

pub struct GitHub {
    agent: ureq::Agent,
    tokens: Mutex<HashMap<String, Option<String>>>,
    pub offline: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    Gh,
    VsCode,
    None,
}

pub struct Resp {
    pub status: u16,
    pub body: Vec<u8>,
}

impl GitHub {
    pub fn new(offline: bool) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent("new-tricks")
            .build()
            .into();
        GitHub { agent, tokens: Mutex::new(HashMap::new()), offline }
    }

    pub fn api_base(host: &str) -> String {
        if host == "github.com" { "https://api.github.com".into() } else { format!("https://{host}/api/v3") }
    }

    pub fn raw_base(host: &str) -> String {
        if host == "github.com" { "https://raw.githubusercontent.com".into() } else { format!("https://{host}/raw") }
    }

    /// Resolve a token for `host` without storing it anywhere but this process.
    pub fn token(&self, host: &str) -> Option<String> {
        let mut cache = self.tokens.lock().unwrap();
        if let Some(t) = cache.get(host) {
            return t.clone();
        }
        let t = resolve_token(host).map(|(t, _)| t);
        cache.insert(host.to_string(), t.clone());
        t
    }

    pub fn token_source(&self, host: &str) -> TokenSource {
        resolve_token(host).map(|(_, s)| s).unwrap_or(TokenSource::None)
    }

    pub fn get(&self, host: &str, url: &str, accept: Option<&str>) -> Result<Resp> {
        if self.offline {
            bail!("offline");
        }
        let mut req = self.agent.get(url).header("Accept", accept.unwrap_or("application/vnd.github+json"));
        if let Some(t) = self.token(host) {
            req = req.header("Authorization", &format!("Bearer {t}"));
        }
        let mut resp = req.call().map_err(|e| anyhow!("GET {url}: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().with_config().limit(50 * 1024 * 1024).read_to_vec().map_err(|e| anyhow!("reading {url}: {e}"))?;
        Ok(Resp { status, body })
    }

    /// GET a URL without GitHub credentials (third-party catalogs).
    pub fn get_public(&self, url: &str) -> Result<Resp> {
        if self.offline {
            bail!("offline");
        }
        let mut resp = self.agent.get(url).header("Accept", "application/json").call().map_err(|e| anyhow!("GET {url}: {e}"))?;
        let status = resp.status().as_u16();
        let body = resp.body_mut().with_config().limit(20 * 1024 * 1024).read_to_vec().map_err(|e| anyhow!("reading {url}: {e}"))?;
        Ok(Resp { status, body })
    }

    pub fn api_json<T: DeserializeOwned>(&self, host: &str, path: &str) -> Result<T> {
        let url = format!("{}{}", Self::api_base(host), path);
        let r = self.get(host, &url, None)?;
        match r.status {
            200 => Ok(serde_json::from_slice(&r.body)?),
            401 => bail!("GitHub authentication failed for {host} (run `gh auth login`)"),
            403 | 429 => bail!(
                "GitHub API rate limit or permission error for {path} ({}); authenticate with `gh auth login` for higher limits",
                r.status
            ),
            404 => Err(anyhow!(NotFound(path.to_string()))),
            s => bail!("GitHub API {path} returned {s}: {}", String::from_utf8_lossy(&r.body).chars().take(200).collect::<String>()),
        }
    }

    pub fn raw_file(&self, host: &str, repo: &str, commit: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let url = format!("{}/{repo}/{commit}/{}", Self::raw_base(host), encode_path(path));
        let r = self.get(host, &url, Some("*/*"))?;
        match r.status {
            200 => Ok(Some(r.body)),
            404 => Ok(None),
            s => bail!("fetching {url} returned {s}"),
        }
    }

    /// The authenticated user's login and organization logins (for the trust facet
    /// and the `auto` policy check). Environment override for tests and CI.
    pub fn identity(&self, host: &str) -> Result<(String, Vec<String>)> {
        if let Ok(v) = std::env::var("TRICKS_IDENTITY") {
            let mut parts = v.split(',').map(|s| s.trim().to_string());
            let login = parts.next().unwrap_or_default();
            return Ok((login, parts.collect()));
        }
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        #[derive(Deserialize)]
        struct Org {
            login: String,
        }
        let u: User = self.api_json(host, "/user")?;
        let orgs: Vec<Org> = self.api_json(host, "/user/orgs?per_page=100").unwrap_or_default();
        Ok((u.login, orgs.into_iter().map(|o| o.login).collect()))
    }
}

#[derive(Debug)]
pub struct NotFound(pub String);
impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "not found: {}", self.0)
    }
}
impl std::error::Error for NotFound {}

pub fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<NotFound>().is_some()
}

fn resolve_token(host: &str) -> Option<(String, TokenSource)> {
    for var in ["TRICKS_GITHUB_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(t) = std::env::var(var)
            && !t.trim().is_empty()
        {
            return Some((t.trim().to_string(), TokenSource::Env));
        }
    }
    if std::env::var_os("TRICKS_NO_GH").is_none()
        && let Ok(out) = Command::new("gh").args(["auth", "token", "--hostname", host]).output()
        && out.status.success()
    {
        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !t.is_empty() {
            return Some((t, TokenSource::Gh));
        }
    }
    // Session handed over in memory by the VS Code extension (github.com only).
    if host == "github.com"
        && let Ok(t) = std::env::var("TRICKS_VSCODE_TOKEN")
        && !t.trim().is_empty()
    {
        return Some((t.trim().to_string(), TokenSource::VsCode));
    }
    None
}

pub fn encode_path(p: &str) -> String {
    p.split('/')
        .map(|seg| {
            seg.bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                    _ => format!("%{b:02X}"),
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn encode_query(q: &str) -> String {
    q.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
